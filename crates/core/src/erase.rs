//! 個人情報の論理消去と、全文索引・保存ファイルの圧縮。

const ERASED_COMPANY_NAME: &str = "(消去済み)";
const ERASE_COMPANY_CONTACTS: &str = "UPDATE contacts SET name=NULL,phone=NULL,email=NULL,erased_at=?2 WHERE company_id=?1 AND erased_at IS NULL";
const ERASE_COMPANY: &str = "UPDATE companies SET name=?3,name_key=?4,corporate_number=NULL,phone=NULL,website=NULL,owner_id=NULL,industry_code=NULL,source_id=NULL,source_as_of=NULL,prefecture_code=0,municipality_code=NULL,erased_at=?2,updated_at=?2 WHERE id=?1";

use crate::sql::OptionalExtension;
use crate::{
    normalize,
    schema::{Db, WRITE_BASE, counts, ensure_space, fts, with_erase_gate},
    types::{
        ActivityInput, ActivityKind, CompanyId, ContactId, DataOrigin, EngineError, NextAction,
        Status,
    },
    write::{now_secs, record_activity_tx},
};
use std::{fs, path::Path};

fn storage(error: impl std::error::Error + Send + Sync + 'static) -> EngineError {
    EngineError::Storage(Box::new(error))
}

const CONTACT_ERASED_AT: &str = "SELECT erased_at FROM contacts WHERE id=?1";
const ERASE_CONTACT: &str =
    "UPDATE contacts SET name=NULL,phone=NULL,email=NULL,erased_at=?2 WHERE id=?1";
const ERASE_CONTACT_NOTES: &str =
    "UPDATE activities SET note=NULL,note_erased_at=?2 WHERE contact_id=?1 AND note IS NOT NULL";
const COMPANY_ERASURE_STATE: &str =
    "SELECT is_sole_proprietor,erased_at,name_key,has_phone FROM companies WHERE id=?1";
const ERASE_COMPANY_NOTES: &str =
    "UPDATE activities SET note=NULL,note_erased_at=?2 WHERE company_id=?1 AND note IS NOT NULL";

const ACTIVITY_COUNT: &str = "SELECT count(*) FROM activities WHERE company_id=?1";
const CONTACT_COUNT: &str = "SELECT count(*) FROM contacts WHERE company_id=?1";

pub(crate) fn erase_contact_estimate(page_size: u64, activities: u64) -> u64 {
    WRITE_BASE.saturating_add(page_size.saturating_mul(activities.saturating_add(8)))
}

pub(crate) fn erase_company_estimate(
    page_size: u64,
    activities: u64,
    contacts: u64,
    key: &str,
) -> u64 {
    erase_contact_estimate(page_size, activities.saturating_add(contacts))
        .saturating_add((key.len() as u64).saturating_mul(16))
}

fn page_size(db: &Db) -> Result<u64, EngineError> {
    db.conn()
        .pragma_query_value(None, "page_size", |r| r.get::<_, u32>(0))
        .map(u64::from)
        .map_err(storage)
}

fn company_count(db: &Db, sql: &str, company: i64) -> Result<u64, EngineError> {
    let count: i64 = db
        .conn()
        .query_row(sql, [company], |r| r.get(0))
        .map_err(storage)?;
    u64::try_from(count).map_err(|_| EngineError::Corrupt)
}

// CRM消去triggerが更新する行と旧テキスト、索引/WALを保守的に見込む。
fn crm_erase_estimate(db: &Db, key: &str, id: i64) -> Result<u64, EngineError> {
    if db.origin() != DataOrigin::Synthetic {
        return Ok(0);
    }
    let mut rows = 0_u64;
    let mut bytes = 0_u64;
    for (table, text) in [("crm_work_items", "subject"), ("crm_opportunities", "name")] {
        let (count, length): (i64, i64) = db
            .conn()
            .query_row(
                &format!(
                    "SELECT count(*),coalesce(sum(length(CAST({text} AS BLOB))),0) FROM {table} WHERE {key}=?1"
                ),
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(storage)?;
        rows = rows.saturating_add(u64::try_from(count).map_err(|_| EngineError::Corrupt)?);
        bytes = bytes.saturating_add(u64::try_from(length).map_err(|_| EngineError::Corrupt)?);
    }
    if rows == 0 {
        return Ok(0);
    }
    Ok(page_size(db)?
        .saturating_mul(rows.saturating_mul(8).saturating_add(16))
        .saturating_add(bytes.saturating_mul(8)))
}

pub(crate) fn erase_contact(
    db: &mut Db,
    id: ContactId,
    wipe_notes: bool,
) -> Result<(), EngineError> {
    let estimate = db.local_growth_estimate(|| {
        // contact_idには索引がないため、企業の活動数で上限を見込む。
        let company: Option<i64> = db
            .conn()
            .query_row("SELECT company_id FROM contacts WHERE id=?1", [id.0], |r| {
                r.get(0)
            })
            .optional()
            .map_err(storage)?;
        Ok(match company {
            Some(company) => {
                erase_contact_estimate(page_size(db)?, company_count(db, ACTIVITY_COUNT, company)?)
            }
            None => WRITE_BASE,
        }
        .saturating_add(crm_erase_estimate(db, "contact_id", id.0)?))
    })?;
    db.write(estimate, |tx| {
        let erased: Option<i64> = tx
            .query_row(CONTACT_ERASED_AT, [id.0], |r| r.get(0))
            .optional()
            .map_err(storage)?
            .ok_or(EngineError::NotFound)?;
        if erased.is_some() && !wipe_notes {
            return Ok(());
        }
        with_erase_gate(tx, |tx| {
            let now = now_secs();
            if erased.is_none() {
                tx.execute(ERASE_CONTACT, (id.0, now)).map_err(storage)?;
            }
            if wipe_notes {
                tx.execute(ERASE_CONTACT_NOTES, (id.0, now))
                    .map_err(storage)?;
            }
            Ok(())
        })
    })
}

pub(crate) fn erase_company(db: &mut Db, id: CompanyId) -> Result<(), EngineError> {
    let estimate = db.local_growth_estimate(|| {
        let key: Option<String> = db
            .conn()
            .query_row("SELECT name_key FROM companies WHERE id=?1", [id.0], |r| {
                r.get(0)
            })
            .optional()
            .map_err(storage)?;
        Ok(match key {
            Some(key) => erase_company_estimate(
                page_size(db)?,
                company_count(db, ACTIVITY_COUNT, id.0)?,
                company_count(db, CONTACT_COUNT, id.0)?,
                &key,
            ),
            None => WRITE_BASE,
        }
        .saturating_add(crm_erase_estimate(db, "company_id", id.0)?)
        .saturating_add(crate::custom_fields::erase_estimate(db, id)?))
    })?;
    db.write(estimate, |tx| {
        let (sole, erased, key, phone): (bool, Option<i64>, String, bool) = tx
            .query_row(COMPANY_ERASURE_STATE, [id.0], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()
            .map_err(storage)?
            .ok_or(EngineError::NotFound)?;
        if !sole {
            return Err(EngineError::InvalidInput {
                field: "id".into(),
                reason: "not a sole proprietor".into(),
            });
        }
        if erased.is_some() {
            return Ok(());
        }
        with_erase_gate(tx, |tx| {
            let now = now_secs();
            record_activity_tx(
                tx,
                &ActivityInput {
                    company: id,
                    contact: None,
                    kind: ActivityKind::StatusChange,
                    at: now,
                    note: None,
                    status_after: Some(Status::Excluded),
                    next_action: NextAction::Clear,
                    reverses: None,
                },
                true,
            )?;
            tx.execute(ERASE_COMPANY_CONTACTS, (id.0, now))
                .map_err(storage)?;
            tx.execute(ERASE_COMPANY_NOTES, (id.0, now))
                .map_err(storage)?;
            crate::custom_fields::erase_values(tx, id)?;
            crate::public_data::remember_erasure(tx, id)?;
            let erased_key = normalize::name_key(ERASED_COMPANY_NAME);
            tx.execute(ERASE_COMPANY, (id.0, now, ERASED_COMPANY_NAME, &erased_key))
                .map_err(storage)?;
            fts::delete(tx, id.0, &key)?;
            fts::insert(tx, id.0, &erased_key)?;
            counts::apply_delta(
                tx,
                Some((Status::Excluded, phone)),
                Some((Status::Excluded, false)),
            )?;
            Ok(())
        })
    })
}

pub(crate) fn compact(db: &mut Db) -> Result<(), EngineError> {
    #[cfg(feature = "cloud")]
    if !db.has_local_storage() {
        return Err(storage(meibo_db::RemoteErrorKind::Unsupported.error()));
    }
    let path = Path::new(db.conn().path().ok_or_else(|| EngineError::InvalidInput {
        field: "path".into(),
        reason: "file database required".into(),
    })?);
    // 未チェックポイントのWALに増えたページも必要量に含める。
    let pages: u32 = db
        .conn()
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(storage)?;
    let page_size: u32 = db
        .conn()
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .map_err(storage)?;
    let size = fs::metadata(path)
        .map_err(storage)?
        .len()
        .max(u64::from(pages) * u64::from(page_size));
    ensure_space(
        path,
        size.checked_mul(2).ok_or(EngineError::InsufficientSpace)?,
        db.space_probe(),
    )?;
    fts::rebuild(db.conn())?;
    db.conn().execute_batch("VACUUM").map_err(storage)?;
    truncate_checkpoint(db)
}

/// 保存先と WAL を物理的に縮める。背景のチェックポイントが同時に動いていると、SQLite は
/// 待たずに「忙しい」を返す。接続の待ち時間（`busy_timeout`）まで短い間隔で再試行する。
fn truncate_checkpoint(db: &Db) -> Result<(), EngineError> {
    if crate::schema::checkpoint::retry(db.conn(), crate::schema::checkpoint::Mode::Truncate)? {
        Ok(())
    } else {
        Err(EngineError::CompactIncomplete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lists::tests::{activity, consistent, database, scalar, seed, snapshot},
        types::ContactInput,
        write,
    };

    fn contact(db: &mut Db, company: i64) -> ContactId {
        write::add_contact(
            db,
            &ContactInput {
                company: CompanyId(company),
                name: "synthetic person".into(),
                phone: Some("0123".into()),
                email: Some("synthetic@example.invalid".into()),
            },
        )
        .unwrap()
    }

    fn note(db: &mut Db, company: i64, contact: ContactId) {
        write::record_activity(
            db,
            &ActivityInput {
                company: CompanyId(company),
                contact: Some(contact),
                kind: ActivityKind::Memo,
                at: 101,
                note: Some("zqxjvk7".into()),
                status_after: None,
                next_action: NextAction::Keep,
                reverses: None,
            },
        )
        .unwrap();
    }

    // REQ 7.3
    // REQ 5.3
    #[test]
    fn contact_erasure_optional_notes_and_idempotence() {
        let (_dir, mut db) = database();
        seed(&mut db, 2);
        let a = contact(&mut db, 1);
        let b = contact(&mut db, 1);
        note(&mut db, 1, a);
        note(&mut db, 1, b);
        activity(&mut db, 1, None);
        erase_contact(&mut db, a, false).unwrap();
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM contacts WHERE id=1 AND name IS NULL AND phone IS NULL AND email IS NULL AND erased_at IS NOT NULL AND company_id=1 AND created_at IS NOT NULL"
            ),
            1
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE note IS NOT NULL"
            ),
            3
        );
        // 日時は秒単位で同じ秒だと区別できない。過去の値に置いて、上書きを検出する。
        db.conn()
            .execute("UPDATE contacts SET erased_at=1 WHERE id=1", [])
            .unwrap();
        let erased_at = scalar(&db, "SELECT erased_at FROM contacts WHERE id=1");
        assert_eq!(erased_at, 1);
        erase_contact(&mut db, a, true).unwrap();
        assert_eq!(
            scalar(&db, "SELECT erased_at FROM contacts WHERE id=1"),
            erased_at
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE contact_id=1 AND note IS NULL AND note_erased_at IS NOT NULL"
            ),
            1
        );
        let before = snapshot(db.conn());
        erase_contact(&mut db, a, true).unwrap();
        assert_eq!(snapshot(db.conn()), before);
        erase_contact(&mut db, b, true).unwrap();
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE contact_id=2 AND note IS NULL AND note_erased_at IS NOT NULL"
            ),
            1
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE note IS NOT NULL"
            ),
            1
        );
        let before = snapshot(db.conn());
        erase_contact(&mut db, b, true).unwrap();
        assert_eq!(snapshot(db.conn()), before);
        assert!(matches!(
            erase_contact(&mut db, ContactId(999), true),
            Err(EngineError::NotFound)
        ));
        consistent(&db);
    }

    // REQ 7.2
    // REQ 7.5
    // REQ 5.3
    #[test]
    fn sole_proprietor_erasure_preserves_only_contract_fields_and_folds() {
        let (_dir, mut db) = database();
        seed(&mut db, 2);
        db.conn().execute_batch("UPDATE companies SET corporate_number='6600000000000',municipality_code='13101',industry_code=1,website='https://synthetic.invalid',source_id='synthetic',source_as_of='2024-01-01' WHERE id=2").unwrap();
        let a = contact(&mut db, 2);
        let b = contact(&mut db, 2);
        note(&mut db, 2, a);
        note(&mut db, 2, b);
        erase_contact(&mut db, a, true).unwrap();
        activity(&mut db, 2, Some(Status::Won));
        erase_company(&mut db, CompanyId(2)).unwrap();
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM companies WHERE id=2 AND corporate_number IS NULL AND phone IS NULL AND website IS NULL AND owner_id IS NULL AND industry_code IS NULL AND source_id IS NULL AND source_as_of IS NULL AND prefecture_code=0 AND municipality_code IS NULL AND erased_at IS NOT NULL AND updated_at=erased_at AND created_at=10 AND is_sole_proprietor=1 AND base_status=0 AND status=5 AND last_activity_at=101 AND next_action_at IS NULL AND has_phone=0"
            ),
            1
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM contacts WHERE company_id=2 AND name IS NULL AND phone IS NULL AND email IS NULL AND erased_at IS NOT NULL"
            ),
            2
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE company_id=2 AND note IS NOT NULL"
            ),
            0
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE note_erased_at IS NOT NULL"
            ),
            3
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE company_id=2 AND kind=5 AND status_after=5 AND next_action_kind=1"
            ),
            1
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM name_fts WHERE name_fts MATCH '\"synthetic2\"'"
            ),
            0
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM name_fts WHERE name_fts MATCH '\"synthetic1\"'"
            ),
            1
        );
        let (name, key): (String, String) = db
            .conn()
            .query_row("SELECT name,name_key FROM companies WHERE id=2", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(name, ERASED_COMPANY_NAME);
        assert_eq!(key, normalize::name_key(ERASED_COMPANY_NAME));
        let before = snapshot(db.conn());
        erase_company(&mut db, CompanyId(2)).unwrap();
        assert_eq!(snapshot(db.conn()), before);
        consistent(&db);
    }

    // REQ 7.2
    // REQ 7.5
    #[test]
    fn erasure_rejects_missing_and_corporate_and_rolls_back_failures() {
        let (_dir, mut db) = database();
        seed(&mut db, 2);
        db.conn()
            .execute("UPDATE companies SET is_sole_proprietor=0 WHERE id=1", [])
            .unwrap();
        let before = snapshot(db.conn());
        assert!(
            matches!(erase_company(&mut db,CompanyId(1)),Err(EngineError::InvalidInput{field,reason}) if field=="id" && reason=="not a sole proprietor")
        );
        assert!(matches!(
            erase_company(&mut db, CompanyId(999)),
            Err(EngineError::NotFound)
        ));
        assert_eq!(snapshot(db.conn()), before);
        let a = contact(&mut db, 2);
        note(&mut db, 2, a);
        db.conn().execute_batch("CREATE TEMP TRIGGER fail_erase BEFORE UPDATE OF name ON companies BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
        let before = snapshot(db.conn());
        assert!(erase_company(&mut db, CompanyId(2)).is_err());
        assert_eq!(snapshot(db.conn()), before);
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM name_fts WHERE name_fts MATCH '\"synthetic2\"'"
            ),
            1
        );
        consistent(&db);
    }

    fn files(db: &Db) -> Vec<Vec<u8>> {
        let path = db.conn().path().unwrap();
        [path.to_owned(), format!("{path}-wal")]
            .iter()
            .map(|p| match fs::read(p) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => panic!("{e}"),
            })
            .collect()
    }

    // REQ 7.3
    // REQ 7.5
    #[test]
    fn compact_removes_erased_words_and_trigrams_from_database_and_wal() {
        let (_dir, mut db) = database();
        seed(&mut db, 2);
        db.conn()
            .execute_batch("PRAGMA secure_delete=OFF; PRAGMA wal_autocheckpoint=0")
            .unwrap();
        // 消去は主ファイルとWALの固定の余裕を見込む。
        db.write(WRITE_BASE, |tx| {
            fts::delete(tx, 2, "synthetic2")?;
            tx.execute(
                "UPDATE companies SET name='zqxjvk7',name_key='zqxjvk7' WHERE id=2",
                [],
            )
            .map_err(storage)?;
            fts::insert(tx, 2, "zqxjvk7")
        })
        .unwrap();
        let a = contact(&mut db, 2);
        note(&mut db, 2, a);
        db.conn()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        erase_company(&mut db, CompanyId(2)).unwrap();
        assert!(
            files(&db)
                .iter()
                .any(|data| data.windows(7).any(|s| s == b"zqxjvk7"))
        );
        compact(&mut db).unwrap();
        for data in files(&db) {
            for word in [
                b"zqxjvk7".as_slice(),
                b"zqx",
                b"xjv",
                b"qxj",
                b"jvk",
                b"vk7",
            ] {
                assert!(
                    !data.windows(word.len()).any(|s| s == word),
                    "remaining {word:?}"
                );
            }
        }
        consistent(&db);
    }

    // REQ 7.3
    // REQ 7.5
    #[test]
    fn compact_reports_busy_reader_and_succeeds_after_release() {
        let (_dir, mut db) = database();
        seed(&mut db, 2);
        let reader = crate::sql::Connection::open(db.conn().path().unwrap()).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM companies")
            .unwrap();
        db.conn()
            .busy_timeout(std::time::Duration::from_millis(10))
            .unwrap();
        erase_company(&mut db, CompanyId(2)).unwrap();
        assert!(matches!(
            compact(&mut db),
            Err(EngineError::CompactIncomplete)
        ));
        reader.execute_batch("ROLLBACK").unwrap();
        compact(&mut db).unwrap();
        consistent(&db);
    }
}
