//! Public corporate information, kept separate from user-owned sales information.
use crate::sql::{OptionalExtension, params};
use crate::{
    Engine,
    types::{CompanyInput, DataOrigin, EngineError, Status},
};
use crate::{
    bulk,
    schema::{WRITE_BASE, counts, fts},
    write,
};
use sha2::{Digest, Sha256};

pub(crate) const VERSION: u32 = 1;

pub(crate) fn initialize(tx: &crate::sql::Transaction<'_>) -> Result<(), EngineError> {
    let version: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='public_data_schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(version) = version {
        let found = version.parse().map_err(|_| EngineError::Corrupt)?;
        if found != VERSION {
            return Err(EngineError::UnsupportedVersion {
                found,
                supported: VERSION,
            });
        }
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS erased_corporate_numbers(number_hash BLOB PRIMARY KEY);",
    )
    .map_err(storage)?;
    tx.execute(
        "INSERT OR IGNORE INTO meta(key,value) VALUES('public_data_schema_version',?1)",
        [VERSION.to_string()],
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) fn remember_erasure(
    tx: &crate::sql::Transaction<'_>,
    id: crate::types::CompanyId,
) -> Result<(), EngineError> {
    let number: Option<String> = tx
        .query_row(
            "SELECT corporate_number FROM companies WHERE id=?1",
            [id.0],
            |r| r.get(0),
        )
        .map_err(storage)?;
    if let Some(number) = number {
        tx.execute(
            "INSERT OR IGNORE INTO erased_corporate_numbers(number_hash) VALUES(?1)",
            [Sha256::digest(number.as_bytes()).as_slice()],
        )
        .map_err(storage)?;
    }
    Ok(())
}

/// Attribution used wherever the national corporate directory is presented.
pub const NTA_ATTRIBUTION: &str =
    "出典：国税庁法人番号公表サイト https://www.houjin-bangou.nta.go.jp/";
/// Notice that the source has been converted to the CRM's fields.
pub const NTA_MODIFICATION: &str =
    "国税庁の公表データを加工して作成。国税庁による製品の推奨を意味しません。";

/// Only the public fields the importer may refresh.
#[derive(Debug, Clone)]
pub struct PublicCompanyInput {
    /// Valid 13-digit corporate number.
    pub corporate_number: String,
    /// Published legal name.
    pub name: String,
    /// Prefecture code, 1 through 47.
    pub prefecture: u8,
    /// Source date, YYYY-MM-DD.
    pub source_as_of: String,
}

/// Counts from committed batches only.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PublicImportReport {
    /// Newly registered companies.
    pub inserted: u64,
    /// Existing public records refreshed.
    pub updated: u64,
    /// Erased records left unchanged.
    pub skipped: u64,
}

impl Engine {
    /// Refresh at most 500 public records without modifying sales-owned fields.
    /// # Errors
    /// Rejects synthetic destinations, invalid records and storage failures atomically.
    pub fn upsert_public_companies(
        &mut self,
        rows: &[PublicCompanyInput],
    ) -> Result<PublicImportReport, EngineError> {
        self.db.check_origin(DataOrigin::Real)?;
        if rows.len() > 500 || rows.iter().any(|r| r.name.len() > 4000) {
            return Err(EngineError::InvalidInput {
                field: "public_data".into(),
                reason: "batch or name exceeds limit".into(),
            });
        }
        let normalized = rows
            .iter()
            .map(|r| write::validate_company_input(&company(r)))
            .collect::<Result<Vec<_>, _>>()?;
        let estimate = normalized.iter().fold(WRITE_BASE, |n, r| {
            n.saturating_add(write::company_estimate(&r.company))
        });
        self.db.write(estimate, |tx| {
            let mut report=PublicImportReport::default();
            let mut insert=tx.prepare_cached(bulk::INSERT_COMPANY).map_err(storage)?;
            let has_erased:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM erased_corporate_numbers)",[],|r|r.get(0)).map_err(storage)?;
            for row in &normalized {
                let c=&row.company;
                if has_erased {
                    let hash=Sha256::digest(c.corporate_number.as_deref().unwrap_or_default().as_bytes());
                    let erased:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM erased_corporate_numbers WHERE number_hash=?1)",[hash.as_slice()],|r|r.get(0)).map_err(storage)?;
                    if erased {report.skipped+=1;continue;}
                }
                let previous: Option<(i64,String,bool,Option<String>)> = tx.query_row(
                    "SELECT id,name_key,erased_at IS NOT NULL,source_as_of FROM companies WHERE corporate_number=?1",
                    [&c.corporate_number], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
                ).optional().map_err(storage)?;
                if let Some((id,old_key,erased,date))=previous {
                    if erased || date.as_deref().zip(c.source_as_of.as_deref()).is_some_and(|(old,new)| old > new) {
                        report.skipped+=1;
                        continue;
                    }
                    if old_key != row.name_key { fts::delete(tx,id,&old_key)?; }
                    tx.execute("UPDATE companies SET name=?2,name_key=?3,municipality_code=CASE WHEN prefecture_code=?4 THEN municipality_code ELSE NULL END,prefecture_code=?4,source_id='nta',source_as_of=?5,updated_at=?6 WHERE id=?1",
                        params![id,c.name,row.name_key,c.prefecture,c.source_as_of,write::now_secs()]).map_err(storage)?;
                    if old_key != row.name_key { fts::insert(tx,id,&row.name_key)?; }
                    report.updated+=1;
                } else {
                    let id=bulk::insert_company(tx,&mut insert,row)?;
                    fts::insert(tx,id,&row.name_key)?;
                    counts::apply_delta(tx,None,Some((Status::Untouched,false)))?;
                    report.inserted+=1;
                }
            }
            Ok(report)
        })
    }
}

fn storage(error: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(error))
}

fn company(row: &PublicCompanyInput) -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Real,
        corporate_number: Some(row.corporate_number.clone()),
        name: row.name.clone(),
        prefecture: row.prefecture,
        municipality: None,
        industry: None,
        phone: None,
        website: None,
        owner: None,
        status: Status::Untouched,
        is_sole_proprietor: false,
        source_id: Some("nta".into()),
        source_as_of: Some(row.source_as_of.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        OpenOptions,
        types::{ActivityInput, ActivityKind, Fields, NextAction, UserId},
    };

    fn row() -> PublicCompanyInput {
        PublicCompanyInput {
            corporate_number: "6600000000000".into(),
            name: "合成公的情報".into(),
            prefecture: 13,
            source_as_of: "2026-10-23".into(),
        }
    }

    #[test]
    fn refresh_preserves_sales_state_phone_owner_and_activity() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::create(
            &dir.path().join("real.db"),
            DataOrigin::Real,
            &OpenOptions::default(),
        )
        .unwrap();
        let r = row();
        assert_eq!(
            engine
                .upsert_public_companies(std::slice::from_ref(&r))
                .unwrap()
                .inserted,
            1
        );
        let id = crate::types::CompanyId(1);
        let mut original = company(&r);
        original.phone = Some("0312345678".into());
        original.owner = Some(UserId(7));
        engine.update_company(id, &original).unwrap();
        engine
            .record_activity(&ActivityInput {
                company: id,
                contact: None,
                kind: ActivityKind::Call,
                at: 100,
                note: Some("営業メモ".into()),
                status_after: Some(Status::Negotiating),
                next_action: NextAction::Set(200),
                reverses: None,
            })
            .unwrap();
        let mut updated = r;
        updated.name = "合成名称変更".into();
        assert_eq!(
            engine.upsert_public_companies(&[updated]).unwrap().updated,
            1
        );
        let detail = engine.company_detail(id, &Fields::new()).unwrap();
        assert_eq!(detail.company.name.as_deref(), Some("合成名称変更"));
        assert_eq!(detail.company.status, Status::Negotiating);
        assert_eq!(detail.company.next_action_at, Some(200));
        assert_eq!(detail.company.owner, Some(UserId(7)));
        assert_eq!(detail.company.phone.as_deref(), Some("0312345678"));
        assert_eq!(detail.activities.len(), 1);
        assert!(engine.verify().unwrap().counts_match);
    }

    #[test]
    fn bad_record_rolls_back_the_entire_batch_and_origin_stays_separate() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::create(
            &dir.path().join("real.db"),
            DataOrigin::Real,
            &OpenOptions::default(),
        )
        .unwrap();
        let mut bad = row();
        bad.corporate_number = "invalid".into();
        assert!(engine.upsert_public_companies(&[row(), bad]).is_err());
        assert_eq!(
            engine.count(&crate::types::Filters::default()).unwrap().n,
            0
        );
        let mut synthetic = Engine::create(
            &dir.path().join("synthetic.db"),
            DataOrigin::Synthetic,
            &OpenOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            synthetic.upsert_public_companies(&[row()]),
            Err(EngineError::OriginMismatch)
        ));
    }

    #[test]
    fn public_refresh_does_not_recreate_a_previously_erased_number() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::create(
            &dir.path().join("real.db"),
            DataOrigin::Real,
            &OpenOptions::default(),
        )
        .unwrap();
        let mut original = company(&row());
        original.is_sole_proprietor = true;
        let id = engine.register_company(&original).unwrap();
        engine.erase_company(id).unwrap();
        assert_eq!(engine.upsert_public_companies(&[row()]).unwrap().skipped, 1);
        assert_eq!(
            engine.count(&crate::types::Filters::default()).unwrap().n,
            0
        );
        assert!(engine.verify().unwrap().integrity_ok);
    }
}
