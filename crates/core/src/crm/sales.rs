//! 月単位の実績と見込み。組織DBの同一読み取りトランザクションで集計する。
use std::collections::BTreeMap;

use super::{
    Engine, EngineError, invalid, opportunity_filter, outcome_column, storage, valid_date,
};
use crate::sql::params;
use crate::types::UserId;

/// 指定月に完了予定の未決着商談を、取引先の現在の担当でまとめた額面。
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct OwnerForecast {
    /// 商談の件数。ゼロ円の商談も含む。
    pub count: u64,
    /// 円単位。各商談はJavaScript安全整数、合計はそれを超え得る。
    pub amount_jpy: u128,
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
        let outcome = outcome_column(&tx)?;
        let (visible, _) = opportunity_filter(None);
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
            "SELECT o.id,o.stage,o.amount_jpy,c.owner_id {visible} AND o.id>?3
            AND ((o.stage<4 AND o.close_date BETWEEN ?1 AND ?2)
              OR (o.stage>=4 AND {outcome} BETWEEN ?1 AND ?2)) ORDER BY o.id LIMIT 500"
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
                match stage {
                    0..=3 => {
                        let owner: Option<i64> = row.get(3).map_err(storage)?;
                        let group = report.forecasts.entry(owner.map(UserId)).or_default();
                        group.count += 1;
                        group.amount_jpy += amount;
                    }
                    4 => {
                        report.won_count += 1;
                        report.won_amount_jpy += amount;
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
        tx.rollback().map_err(storage)?;
        Ok(report)
    }
}
