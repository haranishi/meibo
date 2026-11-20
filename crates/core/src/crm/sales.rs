//! 月単位の実績と見込み。組織DBの同一読み取りトランザクションで集計する。
use std::collections::BTreeMap;

use super::{
    Engine, EngineError, forecast::ForecastProjection, invalid, opportunity_filter, outcome_column,
    storage, valid_date, version,
};
use crate::sql::{Connection, params};
use crate::types::UserId;

/// 指定月に完了予定の未決着商談を、取引先の現在の担当でまとめた額面。
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct OwnerForecast {
    /// 商談の件数。ゼロ円の商談も含む。
    pub count: u64,
    /// 円単位。各商談はJavaScript安全整数、合計はそれを超え得る。
    pub amount_jpy: u128,
}

/// One salesperson's pipeline, actual wins and optional monthly target.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SalesProjection {
    /// Actual sales decided this month.
    pub won_amount_jpy: u128,
    /// Face value of all open deals due this month.
    pub open_amount_jpy: u128,
    /// Open deals due this month, including those with unknown probability.
    pub open_count: u64,
    /// Sum of individually rounded, probability-weighted open deals.
    pub weighted_amount_jpy: u128,
    /// Open deals omitted from the weighted amount for lack of a probability.
    pub unknown_probability_count: u64,
    /// Optional monthly target. A zero target is distinct from no target.
    pub target_jpy: Option<u64>,
}

impl SalesProjection {
    /// Actual wins plus probability-weighted open opportunities.
    #[must_use]
    pub fn projected_amount_jpy(&self) -> u128 {
        self.won_amount_jpy + self.weighted_amount_jpy
    }

    /// Projected amount minus target, with exact decimal serialization.
    #[must_use]
    pub fn target_difference_jpy(&self) -> Option<String> {
        let target = u128::from(self.target_jpy?);
        let projected = self.projected_amount_jpy();
        Some(if projected < target {
            format!("-{}", target - projected)
        } else {
            (projected - target).to_string()
        })
    }
}

/// 実際の決着日と完了予定日を区別した組織内の月次集計。
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SalesReport {
    /// 対象月に受注した商談の金額。
    pub won_amount_jpy: u128,
    /// 対象月に受注した件数。
    pub won_count: u64,
    /// 対象月に失注した件数。
    pub lost_count: u64,
    /// 日付が不明でどの月にも計上できない決着済み商談の件数。
    pub undated_count: u64,
    /// 未割当を含む担当別見込み。
    pub forecasts: BTreeMap<Option<UserId>, OwnerForecast>,
    /// Actuals, weighted pipeline and targets by the opportunity's effective owner.
    pub projections: BTreeMap<Option<UserId>, SalesProjection>,
}

impl SalesReport {
    /// 受注率を小数第1位までの百分率で返す。分母がゼロなら未算出。
    #[must_use]
    pub fn win_rate_percent(&self) -> Option<f64> {
        let won = u128::from(self.won_count);
        let decided = won + u128::from(self.lost_count);
        let tenths = (won * 1000 + decided / 2).checked_div(decided)?;
        Some(f64::from(u16::try_from(tenths).ok()?) / 10.0)
    }
}

impl Engine {
    /// 指定月（YYYY-MM）の受注実績・担当別見込みを、表示ページに依存せず計算する。
    /// # Errors
    /// 不正な月、保存形式の破損、読み取り失敗。
    pub fn crm_sales_report(&self, month: &str) -> Result<SalesReport, EngineError> {
        let first = format!("{month}-01");
        if !valid_date(&first) {
            return Err(invalid("month", "expected YYYY-MM"));
        }
        let last = format!("{month}-31");
        let tx = self.db.conn().unchecked_transaction().map_err(storage)?;
        let crm_version = version(&tx)?;
        let outcome = outcome_column(crm_version);
        let projection = ForecastProjection::for_version(crm_version);
        let (visible, _) = opportunity_filter(None, projection.join);
        let unknown_sql = format!("SELECT count(*) {visible} AND o.stage>=4 AND {outcome} IS NULL");
        let unknown: i64 = tx
            .query_row(&unknown_sql, [], |row| row.get(0))
            .map_err(storage)?;
        let mut report = SalesReport {
            undated_count: u64::try_from(unknown).map_err(|_| EngineError::Corrupt)?,
            ..SalesReport::default()
        };
        // Each remote response stays bounded. u128 covers i64::MAX valid rows
        // at the maximum opportunity amount without SQLite SUM or float rounding.
        let sql = format!(
            "SELECT o.id,o.stage,o.amount_jpy,c.owner_id,{},{} {visible} AND o.id>?3
            AND ((o.stage<4 AND o.close_date BETWEEN ?1 AND ?2)
              OR (o.stage>=4 AND {outcome} BETWEEN ?1 AND ?2)) ORDER BY o.id LIMIT 500",
            projection.owner, projection.probability
        );
        let mut statement = tx.prepare(&sql).map_err(storage)?;
        let mut after = 0_i64;
        loop {
            let mut rows = statement
                .query(params![first, last, after])
                .map_err(storage)?;
            let mut count = 0;
            while let Some(row) = rows.next().map_err(storage)? {
                after = row.get(0).map_err(storage)?;
                let stage: u8 = row.get(1).map_err(storage)?;
                let amount = u128::try_from(row.get::<_, i64>(2).map_err(storage)?)
                    .map_err(|_| EngineError::Corrupt)?;
                let owner = row.get::<_, Option<i64>>(4).map_err(storage)?.map(UserId);
                match stage {
                    0..=3 => {
                        let company_owner: Option<i64> = row.get(3).map_err(storage)?;
                        let group = report
                            .forecasts
                            .entry(company_owner.map(UserId))
                            .or_default();
                        group.count += 1;
                        group.amount_jpy += amount;
                        let sales = report.projections.entry(owner).or_default();
                        sales.open_count += 1;
                        sales.open_amount_jpy += amount;
                        match row.get::<_, Option<u8>>(5).map_err(storage)? {
                            Some(probability @ 0..=100) => {
                                sales.weighted_amount_jpy +=
                                    (amount * u128::from(probability) + 50) / 100;
                            }
                            None => sales.unknown_probability_count += 1,
                            Some(_) => return Err(EngineError::Corrupt),
                        }
                    }
                    4 => {
                        report.won_count += 1;
                        report.won_amount_jpy += amount;
                        report.projections.entry(owner).or_default().won_amount_jpy += amount;
                    }
                    5 => report.lost_count += 1,
                    _ => return Err(EngineError::Corrupt),
                }
                count += 1;
            }
            if count < 500 {
                break;
            }
        }
        drop(statement);
        if crm_version >= 3 {
            load_targets(&tx, month, &mut report)?;
        }
        tx.rollback().map_err(storage)?;
        Ok(report)
    }
}

fn load_targets(
    conn: &Connection,
    month: &str,
    report: &mut SalesReport,
) -> Result<(), EngineError> {
    let mut after = 0;
    let mut statement = conn.prepare(
        "SELECT owner_id,amount_jpy FROM crm_sales_targets WHERE month=?1 AND owner_id>?2 ORDER BY owner_id LIMIT 500"
    ).map_err(storage)?;
    loop {
        let mut rows = statement.query(params![month, after]).map_err(storage)?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(storage)? {
            after = row.get::<_, i64>(0).map_err(storage)?;
            let amount = u64::try_from(row.get::<_, i64>(1).map_err(storage)?)
                .map_err(|_| EngineError::Corrupt)?;
            report
                .projections
                .entry(Some(UserId(after)))
                .or_default()
                .target_jpy = Some(amount);
            count += 1;
        }
        if count < 500 {
            return Ok(());
        }
    }
}
