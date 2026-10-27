//! 企業と活動を一度の確定で追加する内部の口。

use crate::sql::{OptionalExtension, params};
use crate::{
    schema::{Db, SECONDARY_INDEXES, WRITE_BASE, counts, fts},
    synth::SynthRecord,
    types::{
        ActivityKind, CompanyId, CompanyInput, DataOrigin, DuplicatePolicy, EngineError,
        ImportBatchError, ImportBatchReport, Industry, NextAction, Status,
    },
    write::{self, FoldInput},
};

pub(crate) const INSERT_COMPANY: &str = "INSERT INTO companies(corporate_number,name,name_key,prefecture_code,municipality_code,industry_code,status,phone,website,owner_id,base_status,is_sole_proprietor,source_id,source_as_of,created_at,updated_at,last_activity_at,next_action_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?7,?11,?12,?13,?14,?14,0,NULL)";
const INSERT_ACTIVITY: &str = "INSERT INTO activities(company_id,kind,at,status_after,next_action_kind,next_action_at) VALUES(?1,?2,?3,?4,?5,?6)";

pub(crate) struct BulkRecord {
    pub company: CompanyInput,
    pub activities: Vec<BulkActivity>,
}
pub(crate) struct BulkActivity {
    pub kind: ActivityKind,
    pub at: i64,
    pub status_after: Option<Status>,
    pub next_action: NextAction,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BulkMode {
    Auto,
    #[cfg(test)]
    Incremental,
    Bulk,
}
pub(crate) struct BulkOptions {
    pub mode: BulkMode,
    pub bulk_threshold: u64,
    pub bytes_per_row_estimate: u64,
    pub analyze: bool,
}
impl Default for BulkOptions {
    fn default() -> Self {
        Self {
            mode: BulkMode::Auto,
            bulk_threshold: 50_000,
            bytes_per_row_estimate: 400,
            analyze: true,
        }
    }
}
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct BulkReport {
    pub companies: u64,
    pub activities: u64,
}
impl From<SynthRecord> for BulkRecord {
    fn from(r: SynthRecord) -> Self {
        Self {
            company: r.company,
            activities: r
                .activities
                .into_iter()
                .map(|a| BulkActivity {
                    kind: a.kind,
                    at: a.at,
                    status_after: a.status_after,
                    next_action: a.next_action,
                })
                .collect(),
        }
    }
}
fn storage(e: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(e))
}
fn uses_bulk(total_hint: Option<u64>, opt: &BulkOptions) -> bool {
    opt.mode == BulkMode::Bulk
        || (opt.mode == BulkMode::Auto && total_hint.is_some_and(|n| n >= opt.bulk_threshold))
}

fn estimated_space(total_hint: Option<u64>, opt: &BulkOptions) -> Result<u64, EngineError> {
    total_hint
        .unwrap_or(0)
        .checked_mul(opt.bytes_per_row_estimate)
        .and_then(|n| n.checked_mul(5))
        .map(|n| n / 2)
        .and_then(|n| n.checked_add(64 * 1024 * 1024))
        .ok_or(EngineError::InsufficientSpace)
}

/// 企業を1社追加して内部IDを返す。法人番号の重複は既存の企業を示して拒否する。
pub(crate) fn insert_company(
    tx: &crate::sql::Transaction<'_>,
    stmt: &mut crate::sql::Statement<'_>,
    normalized: &write::NormalizedCompany,
) -> Result<i64, EngineError> {
    let c = &normalized.company;
    let inserted = stmt.execute(params![
        c.corporate_number,
        c.name,
        normalized.name_key,
        c.prefecture,
        c.municipality,
        c.industry.map(Industry::code),
        c.status.code(),
        c.phone,
        c.website,
        c.owner.map(|u| u.0),
        c.is_sole_proprietor,
        c.source_id,
        c.source_as_of,
        write::now_secs()
    ]);
    if let Err(e) = inserted {
        if let Some(number) = &c.corporate_number {
            let existing = tx
                .query_row(FIND_CORPORATE_NUMBER, [number], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(storage)?;
            if let Some(id) = existing {
                return Err(EngineError::DuplicateCorporateNumber {
                    existing: CompanyId(id),
                });
            }
        }
        return Err(storage(e));
    }
    Ok(tx.last_insert_rowid())
}

pub(crate) fn import_companies(
    db: &mut Db,
    companies: &[CompanyInput],
    duplicates: DuplicatePolicy,
) -> Result<ImportBatchReport, ImportBatchError> {
    if companies.len() > 500 {
        return Err(ImportBatchError {
            row: 0,
            error: EngineError::InvalidInput {
                field: "batch".into(),
                reason: "must contain at most 500 companies".into(),
            },
        });
    }
    if companies.is_empty() {
        return Ok(ImportBatchReport::default());
    }
    // 1回の確定の固定余裕は共通にし、入力と正規化した検索キーの分を合算する。
    let estimate = companies.iter().fold(WRITE_BASE, |bytes, company| {
        bytes.saturating_add(write::company_estimate(company).saturating_sub(WRITE_BASE))
    });
    let origin = db.origin();
    let mut row = 0;
    let result = db.write(estimate, |tx| {
        let mut stmt = tx.prepare_cached(INSERT_COMPANY).map_err(storage)?;
        let mut report = ImportBatchReport::default();
        for (index, company) in companies.iter().enumerate() {
            row = index + 1;
            if company.origin != origin {
                return Err(EngineError::OriginMismatch);
            }
            let normalized = write::validate_company_input(company)?;
            let id = match insert_company(tx, &mut stmt, &normalized) {
                Ok(id) => id,
                Err(EngineError::DuplicateCorporateNumber { .. })
                    if duplicates == DuplicatePolicy::Skip =>
                {
                    report.skipped += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            fts::insert(tx, id, &normalized.name_key)?;
            let company = &normalized.company;
            counts::apply_delta(tx, None, Some((company.status, company.phone.is_some())))?;
            report.inserted += 1;
        }
        row = 0;
        Ok(report)
    });
    result.map_err(|error| ImportBatchError { row, error })
}

fn insert_activities(
    stmt: &mut crate::sql::Statement<'_>,
    id: i64,
    status: Status,
    activities: &[BulkActivity],
) -> Result<(), EngineError> {
    let mut current_status = status;
    for a in activities {
        write::validate_activity_times(a.at, a.next_action)?;
        if a.kind == ActivityKind::Cancel {
            return Err(EngineError::InvalidCancel);
        }
        if a.kind == ActivityKind::StatusChange && a.status_after.is_none() {
            return Err(EngineError::InvalidInput {
                field: "status_after".into(),
                reason: "required for status change".into(),
            });
        }
        if current_status == Status::Excluded
            && a.status_after.is_some_and(|s| s != Status::Excluded)
        {
            return Err(EngineError::ExcludedCompany);
        }
        if let Some(s) = a.status_after {
            current_status = s;
        }
        let (next_kind, next_at) = match a.next_action {
            NextAction::Keep => (0, None),
            NextAction::Clear => (1, None),
            NextAction::Set(at) => (2, Some(at)),
        };
        stmt.execute(params![
            id,
            a.kind.code(),
            a.at,
            a.status_after.map(Status::code),
            next_kind,
            next_at
        ])
        .map_err(storage)?;
    }
    Ok(())
}

const UPDATE_FOLDED: &str = "UPDATE companies SET status=?2,last_activity_at=?3,next_action_at=?4,updated_at=?5 WHERE id=?1";
const FIND_CORPORATE_NUMBER: &str = "SELECT id FROM companies WHERE corporate_number=?1";

pub(crate) fn load(
    db: &mut Db,
    records: impl Iterator<Item = BulkRecord>,
    total_hint: Option<u64>,
    opt: &BulkOptions,
) -> Result<BulkReport, EngineError> {
    let needed = db.local_growth_estimate(|| estimated_space(total_hint, opt))?;
    // write は可変借用するため、種別の照合結果を先に固定する。
    let synthetic_ok = db.check_origin(DataOrigin::Synthetic).is_ok();
    let real_ok = db.check_origin(DataOrigin::Real).is_ok();
    let bulk = uses_bulk(total_hint, opt);
    db.write(needed, |tx| {
        if bulk {
            for (name, _) in SECONDARY_INDEXES {
                tx.execute_batch(&format!("DROP INDEX {name};"))
                    .map_err(storage)?;
            }
        }
        let mut report = BulkReport::default();
        {
            let mut company_stmt = tx.prepare_cached(INSERT_COMPANY).map_err(storage)?;
            let mut activity_stmt = tx.prepare_cached(INSERT_ACTIVITY).map_err(storage)?;
            let mut update_stmt = tx.prepare_cached(UPDATE_FOLDED).map_err(storage)?;
            for record in records {
                if !match record.company.origin {
                    DataOrigin::Synthetic => synthetic_ok,
                    DataOrigin::Real => real_ok,
                } {
                    return Err(EngineError::OriginMismatch);
                }
                let normalized = write::validate_company_input(&record.company)?;
                let c = &normalized.company;
                let id = insert_company(tx, &mut company_stmt, &normalized)?;
                insert_activities(&mut activity_stmt, id, c.status, &record.activities)?;
                report.activities += record.activities.len() as u64;
                let folded = write::fold(
                    c.status,
                    record.activities.iter().map(|a| FoldInput {
                        kind: a.kind,
                        at: a.at,
                        status_after: a.status_after,
                        next_action: a.next_action,
                    }),
                );
                if !record.activities.is_empty() {
                    update_stmt
                        .execute(params![
                            id,
                            folded.status.code(),
                            folded.last_activity_at,
                            folded.next_action_at,
                            write::now_secs()
                        ])
                        .map_err(storage)?;
                }
                if !bulk {
                    fts::insert(tx, id, &normalized.name_key)?;
                    counts::apply_delta(tx, None, Some((folded.status, c.phone.is_some())))?;
                }
                report.companies += 1;
            }
        }
        if bulk {
            for (_, sql) in SECONDARY_INDEXES {
                tx.execute_batch(sql).map_err(storage)?;
            }
            fts::rebuild(tx)?;
            counts::recount(tx)?;
            if opt.analyze {
                tx.execute_batch("ANALYZE;").map_err(storage)?;
            }
        }
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::{Connection, types::Value};
    use crate::{
        schema::OpenOptions,
        synth::{Profile, Synth, SynthConfig},
        types::ActivityInput,
    };
    use std::path::Path;
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };
    // REQ 10.3
    // REQ 8.1
    #[test]
    fn estimate_covers_database_and_wal_and_rejects_overflow() {
        let opt = BulkOptions::default();
        assert!(estimated_space(Some(4_445_898), &opt).unwrap() >= 3_435_973_837);
        assert!(matches!(
            estimated_space(Some(u64::MAX), &opt),
            Err(EngineError::InsufficientSpace)
        ));
    }
    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn db(dir: &Path, name: &str) -> Db {
        Db::create(
            &dir.join(name),
            DataOrigin::Synthetic,
            &OpenOptions::default(),
        )
        .unwrap()
    }
    fn config(rows: u64, profile: Profile) -> SynthConfig {
        SynthConfig {
            rows,
            seed: 20_261_022,
            profile,
        }
    }
    fn snapshot(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
        let mut stmt = conn.prepare(sql).unwrap();
        let columns = stmt.column_count();
        stmt.query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    fn indexes(conn: &Connection) -> Vec<Vec<Value>> {
        snapshot(
            conn,
            "SELECT name,sql FROM sqlite_schema WHERE type='index' AND (name GLOB 'k[0-9]*' OR name GLOB 'u[12]*') ORDER BY name",
        )
    }
    fn check(db: &Db) {
        assert!(write::verify_folds(db.conn()).unwrap());
        assert!(counts::matches_recount(db.conn()));
        db.verify_integrity().unwrap();
        db.conn()
            .execute(
                "INSERT INTO name_fts(name_fts,rank) VALUES('integrity-check',1)",
                [],
            )
            .unwrap();
    }
    // REQ 8.1
    #[test]
    fn secondary_indexes_match_ddl() {
        let ddl = include_str!("schema/ddl.sql");
        let actual: Vec<_> = ddl
            .lines()
            .filter(|l| l.starts_with("CREATE INDEX k"))
            .collect();
        assert_eq!(
            actual,
            SECONDARY_INDEXES
                .iter()
                .map(|(_, s)| *s)
                .collect::<Vec<_>>()
        );
    }
    // REQ 8.1
    #[test]
    fn ordinary_incremental_and_bulk_agree() {
        for profile in [Profile::Sparse, Profile::Dense] {
            let dir = temp();
            let c = config(10_000, profile);
            let mut bulk = db(dir.path(), "bulk.db");
            let mut inc = db(dir.path(), "inc.db");
            let mut ordinary = db(dir.path(), "ordinary.db");
            for (db, mode) in [
                (&mut bulk, BulkMode::Bulk),
                (&mut inc, BulkMode::Incremental),
            ] {
                let start = Instant::now();
                let r = load(
                    db,
                    Synth::new(c).map(Into::into),
                    Some(c.rows),
                    &BulkOptions {
                        mode,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert_eq!(r.companies, c.rows);
                let bytes = database_bytes(db.conn());
                eprintln!(
                    "load rows={} profile={profile:?} mode={mode:?} seconds={:.3} db_bytes={bytes}",
                    c.rows,
                    start.elapsed().as_secs_f64()
                );
            }
            let start = Instant::now();
            for r in Synth::new(c) {
                let id = write::register_company(&mut ordinary, &r.company).unwrap();
                for a in r.activities {
                    write::record_activity(
                        &mut ordinary,
                        &ActivityInput {
                            company: id,
                            contact: None,
                            kind: a.kind,
                            at: a.at,
                            note: None,
                            status_after: a.status_after,
                            next_action: a.next_action,
                            reverses: None,
                        },
                    )
                    .unwrap();
                }
            }
            eprintln!(
                "ordinary rows={} profile={profile:?} seconds={:.3} db_bytes={}",
                c.rows,
                start.elapsed().as_secs_f64(),
                database_bytes(ordinary.conn())
            );
            // 壁時計による登録・更新時刻は、各経路の実行時刻なので別に検査する。
            let sql = "SELECT id,corporate_number,name,name_key,prefecture_code,municipality_code,industry_code,status,phone,website,has_phone,owner_id,last_activity_at,next_action_at,base_status,is_sole_proprietor,source_id,source_as_of,erased_at FROM companies ORDER BY id";
            let expected = snapshot(ordinary.conn(), sql);
            for db in [&bulk, &inc, &ordinary] {
                // 大量追加は索引を落として作り直す。成功の後も10本が定義どおりに揃っている。
                for i in 1..=10 {
                    let name = Value::Text(format!("k{i}"));
                    assert!(indexes(db.conn()).iter().any(|r| r[0] == name), "k{i}");
                }
                assert_eq!(indexes(db.conn()), indexes(ordinary.conn()));
                assert_eq!(snapshot(db.conn(), sql), expected);
                assert!(db.conn().query_row("SELECT min(created_at)>0 AND min(updated_at-created_at)>=0 FROM companies",[],|r|r.get::<_,bool>(0)).unwrap());
                assert_eq!(
                    snapshot(db.conn(), "SELECT * FROM counts ORDER BY status,has_phone"),
                    snapshot(
                        ordinary.conn(),
                        "SELECT * FROM counts ORDER BY status,has_phone"
                    )
                );
                assert_eq!(
                    snapshot(db.conn(), "SELECT * FROM activities ORDER BY id"),
                    snapshot(ordinary.conn(), "SELECT * FROM activities ORDER BY id")
                );
                for term in ["テック", "アクア", "サービス"] {
                    let sql = format!(
                        "SELECT rowid FROM name_fts WHERE name_fts MATCH '\"{term}\"' ORDER BY rowid"
                    );
                    assert_eq!(snapshot(db.conn(), &sql), snapshot(ordinary.conn(), &sql));
                }
                check(db);
            }
        }
    }
    // REQ 8.1
    #[test]
    fn failures_roll_back_indexes_and_data() {
        for mode in [BulkMode::Bulk, BulkMode::Incremental] {
            for failure in 0..4 {
                let dir = temp();
                let mut db = db(dir.path(), "rollback.db");
                let before = indexes(db.conn());
                let records = Synth::new(config(1000, Profile::Dense))
                    .enumerate()
                    .map(|(i, r)| {
                        let mut r = BulkRecord::from(r);
                        if i == 499 {
                            match failure {
                                0 => r.company.origin = DataOrigin::Real,
                                1 => r.company.prefecture = 48,
                                2 => {
                                    r.company.corporate_number =
                                        Some(crate::houjin::generate(0).unwrap());
                                }
                                _ => {
                                    r.company.status = Status::Excluded;
                                    r.activities = vec![BulkActivity {
                                        kind: ActivityKind::Call,
                                        at: 1,
                                        status_after: Some(Status::Won),
                                        next_action: NextAction::Keep,
                                    }];
                                }
                            }
                        }
                        r
                    });
                let result = load(
                    &mut db,
                    records,
                    Some(1000),
                    &BulkOptions {
                        mode,
                        ..Default::default()
                    },
                );
                assert!(
                    match failure {
                        0 => matches!(result, Err(EngineError::OriginMismatch)),
                        1 => matches!(result, Err(EngineError::InvalidInput { .. })),
                        2 => matches!(result, Err(EngineError::DuplicateCorporateNumber { .. })),
                        _ => matches!(result, Err(EngineError::ExcludedCompany)),
                    },
                    "{result:?}"
                );
                assert_eq!(counts::total(db.conn()).unwrap(), 0);
                assert_eq!(indexes(db.conn()), before);
                assert_eq!(
                    snapshot(db.conn(), "SELECT * FROM companies"),
                    Vec::<Vec<Value>>::new()
                );
                assert_eq!(
                    snapshot(db.conn(), "SELECT * FROM activities"),
                    Vec::<Vec<Value>>::new()
                );
                check(&db);
            }
        }
    }
    // REQ 8.1
    #[test]
    fn insufficient_space_starts_nothing() {
        let dir = temp();
        let mut db = db(dir.path(), "space.db");
        let before = indexes(db.conn());
        let opt = BulkOptions {
            mode: BulkMode::Bulk,
            ..Default::default()
        };
        let consumed = std::cell::Cell::new(false);
        let records = std::iter::from_fn(|| {
            consumed.set(true);
            None
        });
        assert!(matches!(
            load(&mut db, records, Some(1_000_000_000_000), &opt),
            Err(EngineError::InsufficientSpace)
        ));
        assert!(!consumed.get());
        assert_eq!(indexes(db.conn()), before);
        check(&db);
    }
    // REQ 5.4
    // REQ 8.3
    #[test]
    fn cancel_activity_is_rejected_and_nothing_is_stored() {
        for mode in [BulkMode::Bulk, BulkMode::Incremental] {
            let dir = temp();
            let mut db = db(dir.path(), "cancel.db");
            let mut record =
                BulkRecord::from(Synth::new(config(1, Profile::Dense)).next().unwrap());
            record.activities = vec![BulkActivity {
                kind: ActivityKind::Cancel,
                at: 1,
                status_after: None,
                next_action: NextAction::Keep,
            }];
            let result = load(
                &mut db,
                std::iter::once(record),
                Some(1),
                &BulkOptions {
                    mode,
                    ..Default::default()
                },
            );
            assert!(
                matches!(result, Err(EngineError::InvalidCancel)),
                "{mode:?}"
            );
            assert_eq!(counts::total(db.conn()).unwrap(), 0);
            check(&db);
        }
    }
    // REQ 5.1, 5.4, 8.1
    #[test]
    fn negative_activity_times_roll_back_bulk() {
        for mode in [BulkMode::Bulk, BulkMode::Incremental] {
            for (at, next_action, field) in [
                (-1, NextAction::Keep, "at"),
                (0, NextAction::Set(-1), "next_action"),
            ] {
                let dir = temp();
                let mut db = db(dir.path(), "negative.db");
                let before = crate::lists::tests::snapshot(db.conn());
                let before_indexes = indexes(db.conn());
                let mut record =
                    BulkRecord::from(Synth::new(config(1, Profile::Dense)).next().unwrap());
                record.activities = vec![BulkActivity {
                    kind: ActivityKind::Memo,
                    at,
                    status_after: None,
                    next_action,
                }];
                let result = load(
                    &mut db,
                    std::iter::once(record),
                    Some(1),
                    &BulkOptions {
                        mode,
                        ..Default::default()
                    },
                );
                assert!(
                    matches!(result, Err(EngineError::InvalidInput {field: f, ..}) if f == field)
                );
                assert_eq!(crate::lists::tests::snapshot(db.conn()), before);
                assert_eq!(indexes(db.conn()), before_indexes);
                check(&db);
            }
        }
    }
    // REQ 8.1
    #[test]
    fn auto_selection_and_append_to_existing() {
        let opt = BulkOptions::default();
        assert!(!uses_bulk(None, &opt));
        assert!(!uses_bulk(Some(49_999), &opt));
        assert!(uses_bulk(Some(50_000), &opt));
        let dir = temp();
        let mut db = db(dir.path(), "append.db");
        let c = config(20, Profile::Dense);
        let mut records = Synth::new(c).map(Into::into);
        load(&mut db, records.by_ref().take(10), None, &opt).unwrap();
        load(
            &mut db,
            records,
            Some(10),
            &BulkOptions {
                mode: BulkMode::Bulk,
                ..opt
            },
        )
        .unwrap();
        assert_eq!(counts::total(db.conn()).unwrap(), 20);
        check(&db);
    }
    fn database_bytes(conn: &Connection) -> u64 {
        let pages: i64 = conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        let size: i64 = conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .unwrap();
        u64::try_from(pages * size).unwrap()
    }
    fn benchmark(rows: u64) {
        let dir = temp();
        let path = dir.path().join("bench.db");
        let available = fs4::available_space(dir.path()).unwrap();
        if rows > 100_000 && available < 20 * 1024 * 1024 * 1024 {
            eprintln!("SKIP rows={rows}: available_bytes={available} below 20GiB");
            return;
        }
        let mut db = Db::create(&path, DataOrigin::Synthetic, &OpenOptions::default()).unwrap();
        let peak = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let peak_copy = peak.clone();
        let stop_copy = stop.clone();
        let wal = path.with_file_name("bench.db-wal");
        let monitor = std::thread::spawn(move || {
            while !stop_copy.load(Ordering::Relaxed) {
                if let Ok(m) = fs::metadata(&wal) {
                    peak_copy.fetch_max(m.len(), Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let start = Instant::now();
        let result = load(
            &mut db,
            Synth::new(config(rows, Profile::Dense)).map(Into::into),
            Some(rows),
            &BulkOptions {
                mode: BulkMode::Bulk,
                ..Default::default()
            },
        );
        let elapsed = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        monitor.join().unwrap();
        let report = result.unwrap();
        assert_eq!(report.companies, rows);
        eprintln!(
            "bulk benchmark rows={rows} profile=Dense seconds={:.3} db_bytes={} wal_peak_sampled_bytes={} wal_sample_ms=10 available_before_bytes={available}",
            elapsed.as_secs_f64(),
            database_bytes(db.conn()),
            peak.load(Ordering::Relaxed)
        );
        if rows <= 100_000 {
            check(&db);
        } else {
            assert!(counts::matches_recount(db.conn()));
        }
    }
    // REQ 8.1
    #[test]
    fn dense_100k_timing() {
        benchmark(100_000);
    }
    #[test]
    #[ignore = "445万社の時間・容量の実測"]
    fn dense_4445898_timing() {
        benchmark(4_445_898);
    }
}
