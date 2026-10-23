use super::*;
use crate::types::{OwnerChange, Status, UserId};
use proptest::prelude::*;
use rusqlite::params;
use std::io::{Seek, SeekFrom, Write};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture() -> (TempDir, PathBuf, Db) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.db");
    let db = Db::create(&path, DataOrigin::Synthetic, &OpenOptions::default()).unwrap();
    (dir, path, db)
}

fn company(conn: &Connection, id: i64, key: &str, status: u8, phone: bool) {
    conn.execute(
        "INSERT INTO companies(id,name,name_key,prefecture_code,status,phone,created_at,updated_at) VALUES(?1,?2,?2,13,?3,?4,1,1)",
        params![id,key,status,phone.then_some("000-0000")],
    ).unwrap();
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

// REQ 10.1
// REQ 1.7
// REQ 7.1
#[test]
fn version_one_objects_and_connection_settings() -> TestResult {
    let (_dir, path, db) = fixture();
    for table in [
        "meta",
        "companies",
        "contacts",
        "activities",
        "lists",
        "list_members",
        "counts",
        "erase_gate",
        "name_fts",
    ] {
        assert_eq!(
            db.conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name=?1",
                [table],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
    }
    for index in (1..=10).map(|i| format!("k{i}")).chain(
        [
            "u1_corporate_number",
            "u2_activities_reverses",
            "ix_contacts_company",
            "ix_activities_company",
        ]
        .map(str::to_owned),
    ) {
        assert_eq!(
            db.conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name=?1",
                [index],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
    }
    assert_eq!(
        scalar(
            &db.conn,
            "SELECT count(*) FROM sqlite_schema WHERE type='trigger'"
        ),
        2
    );
    assert_eq!(
        scalar(&db.conn, "SELECT count(*) FROM counts WHERE n=0"),
        12
    );
    assert_eq!(
        scalar(&db.conn, "PRAGMA application_id"),
        i64::from(APPLICATION_ID)
    );
    assert_eq!(scalar(&db.conn, "PRAGMA user_version"), 1);
    let fts_sql: String = db.conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE name='name_fts'",
        [],
        |r| r.get(0),
    )?;
    assert!(fts_sql.contains("tokenize='trigram case_sensitive 1'"));

    assert_eq!(scalar(&db.conn, "PRAGMA synchronous"), 1);
    assert_eq!(scalar(&db.conn, "PRAGMA foreign_keys"), 1);
    assert_eq!(scalar(&db.conn, "PRAGMA cache_size"), -524_288);
    assert_eq!(scalar(&db.conn, "PRAGMA busy_timeout"), 5000);
    assert_eq!(
        db.conn
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?,
        "wal"
    );
    assert!(
        db.conn
            .execute(
                "INSERT INTO contacts(company_id,created_at) VALUES(999,1)",
                []
            )
            .is_err()
    );
    let opt = OpenOptions {
        durability: Durability::Full,
        cache_mib: 64,
        busy_timeout_ms: 17,
        ..OpenOptions::default()
    };
    let other = Db::open(&path, &opt)?;
    assert_eq!(scalar(&other.conn, "PRAGMA synchronous"), 2);
    assert_eq!(scalar(&other.conn, "PRAGMA cache_size"), -65_536);
    assert_eq!(scalar(&other.conn, "PRAGMA busy_timeout"), 17);
    db.verify_integrity()?;
    assert!(Db::create(&path, DataOrigin::Synthetic, &opt).is_err());
    Ok(())
}

// REQ 5.3
// REQ 7.3
#[test]
fn append_only_gate_is_private_and_one_time() -> TestResult {
    let (_dir, path, mut db) = fixture();
    company(&db.conn, 1, "synthetic", 0, false);
    db.conn.execute(
        "INSERT INTO contacts(id,company_id,created_at) VALUES(1,1,1)",
        [],
    )?;
    db.conn.execute("INSERT INTO activities(id,company_id,kind,at,note) VALUES(1,1,4,100,'synthetic note'),(2,1,4,100,'synthetic note')", [])?;
    let reader = Connection::open(&path)?;
    assert!(
        db.conn
            .execute("DELETE FROM activities WHERE id=1", [])
            .is_err()
    );
    assert!(
        db.conn
            .execute("UPDATE activities SET at=101 WHERE id=1", [])
            .is_err()
    );
    assert!(
        db.conn
            .execute(
                "UPDATE activities SET note=NULL,note_erased_at=1 WHERE id=1",
                []
            )
            .is_err()
    );
    db.write(0, |tx| {
        with_erase_gate(tx, |tx| {
            assert_eq!(scalar(&reader, "SELECT count(*) FROM erase_gate"), 0);
            assert!(tx.execute("UPDATE activities SET note=NULL WHERE id=1", []).is_err());
            for assignment in [
                "note='replacement'",
                "id=3",
                "company_id=2",
                "contact_id=1",
                "kind=1",
                "at=101",
                "status_after=1",
                "next_action_kind=1",
                "next_action_at=1",
                "reverses_id=2",
            ] {
                assert!(
                    tx.execute(
                        &format!("UPDATE activities SET note=NULL,note_erased_at=200,{assignment} WHERE id=1"),
                        []
                    )
                    .is_err(),
                    "{assignment}"
                );
            }
            tx.execute(
                "UPDATE activities SET note=NULL,note_erased_at=200 WHERE id=1",
                [],
            )
            .map_err(storage)?;
            assert!(
                tx.execute("UPDATE activities SET note_erased_at=201 WHERE id=1", [])
                    .is_err()
            );
            Ok(())
        })?;
        assert_eq!(scalar(tx, "SELECT count(*) FROM erase_gate"), 0);
        let error: Result<(), EngineError> = with_erase_gate(tx, |_| Err(EngineError::NotFound));
        assert!(matches!(error, Err(EngineError::NotFound)));
        assert_eq!(scalar(tx, "SELECT count(*) FROM erase_gate"), 0);
        Ok(())
    })?;
    assert_eq!(scalar(&reader, "SELECT count(*) FROM erase_gate"), 0);
    assert_eq!(
        scalar(
            &reader,
            "SELECT note IS NULL AND note_erased_at=200 FROM activities WHERE id=1"
        ),
        1
    );
    assert!(
        db.conn
            .execute("UPDATE activities SET note=NULL WHERE id=2", [])
            .is_err()
    );
    Ok(())
}

// REQ 5.4
#[test]
fn cancellation_has_unique_target() -> TestResult {
    let (_dir, _path, db) = fixture();
    company(&db.conn, 1, "synthetic", 0, false);
    db.conn.execute(
        "INSERT INTO activities(id,company_id,kind,at) VALUES(1,1,4,1)",
        [],
    )?;
    db.conn.execute(
        "INSERT INTO activities(company_id,kind,at,reverses_id) VALUES(1,9,2,1)",
        [],
    )?;
    assert!(
        db.conn
            .execute(
                "INSERT INTO activities(company_id,kind,at,reverses_id) VALUES(1,9,3,1)",
                []
            )
            .is_err()
    );
    Ok(())
}

// REQ 8.3
#[test]
fn origin_is_persistent_and_checked_in_both_directions() -> TestResult {
    let (dir, path, db) = fixture();
    assert_eq!(db.origin(), DataOrigin::Synthetic);
    db.check_origin(DataOrigin::Synthetic)?;
    assert!(matches!(
        db.check_origin(DataOrigin::Real),
        Err(EngineError::OriginMismatch)
    ));
    drop(db);
    assert_eq!(
        Db::open(&path, &OpenOptions::default())?.origin(),
        DataOrigin::Synthetic
    );
    // 種別の識別子だけを使い、実データは一切入れない。
    let real = Db::create(
        &dir.path().join("empty-real-origin.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )?;
    real.check_origin(DataOrigin::Real)?;
    assert!(matches!(
        real.check_origin(DataOrigin::Synthetic),
        Err(EngineError::OriginMismatch)
    ));
    Ok(())
}

// REQ 5.2
#[test]
fn write_rolls_back_all_changes_on_error() {
    let (_dir, _path, mut db) = fixture();
    let result: Result<(), EngineError> = db.write(0, |tx| {
        company(tx, 1, "synthetic", 0, true);
        counts::apply_delta(tx, None, Some((Status::Untouched, true)))?;
        fts::insert(tx, 1, "synthetic")?;
        Err(EngineError::NotFound)
    });
    assert!(matches!(result, Err(EngineError::NotFound)));
    assert_eq!(scalar(&db.conn, "SELECT count(*) FROM companies"), 0);
    assert_eq!(counts::total(&db.conn).unwrap(), 0);
    assert_eq!(
        scalar(
            &db.conn,
            "SELECT count(*) FROM name_fts WHERE name_fts MATCH 'synthetic'"
        ),
        0
    );
}

// REQ 3.4
// REQ 6.4
#[test]
fn count_summaries_and_recount_repair() -> TestResult {
    let (_dir, _path, mut db) = fixture();
    db.write(0, |tx| {
        for (id, s, p) in [
            (1, Status::Untouched, true),
            (2, Status::Untouched, false),
            (3, Status::Excluded, true),
        ] {
            company(tx, id, "synthetic", s.code(), p);
            counts::apply_delta(tx, None, Some((s, p)))?;
        }
        Ok(())
    })?;
    assert_eq!(counts::total(&db.conn)?, 3);
    assert_eq!(counts::by_status(&db.conn, Status::Untouched)?, 2);
    assert_eq!(counts::by_phone_excluding_excluded(&db.conn, true)?, 1);
    assert_eq!(counts::by_phone_excluding_excluded(&db.conn, false)?, 1);
    db.conn
        .execute("UPDATE counts SET n=99 WHERE status=0", [])?;
    assert!(!counts::matches_recount(&db.conn));
    counts::recount(&db.conn)?;
    assert!(counts::matches_recount(&db.conn));
    assert_eq!(scalar(&db.conn, "SELECT count(*) FROM counts"), 12);
    // 0件の組から減らす更新は、件数表が壊れている印として拒否する。
    let before = scalar(
        &db.conn,
        "SELECT n FROM counts WHERE status=3 AND has_phone=1",
    );
    assert_eq!(before, 0);
    let result = db.write(0, |tx| {
        counts::apply_delta(tx, Some((Status::Won, true)), None)
    });
    assert!(matches!(result, Err(EngineError::Corrupt)));
    assert_eq!(
        scalar(
            &db.conn,
            "SELECT n FROM counts WHERE status=3 AND has_phone=1"
        ),
        0
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    // REQ 3.4
    // REQ 5.1
    #[test]
    fn count_deltas_match_recount(ops in prop::collection::vec((0usize..8,0u8..6,any::<bool>(),any::<bool>()),1..100)) {
        let (_dir,_path,mut db)=fixture();
        let mut states=[None;8];
        for (slot,code,phone,remove) in ops {
            let old=states[slot];
            let new=(!remove).then_some((Status::from_code(code).unwrap(),phone));
            db.write(0, |tx| {
                tx.execute("DELETE FROM companies WHERE id=?1",[i64::try_from(slot).unwrap()]).map_err(storage)?;
                if let Some((s,p))=new { company(tx,i64::try_from(slot).unwrap(),"synthetic",s.code(),p); }
                counts::apply_delta(tx,old,new)?;
                Ok(())
            }).unwrap();
            states[slot]=new;
            prop_assert!(counts::matches_recount(&db.conn));
            let before: Vec<i64> = db.conn.prepare("SELECT n FROM counts ORDER BY status,has_phone").unwrap().query_map([], |r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            counts::recount(&db.conn).unwrap();
            let after: Vec<i64> = db.conn.prepare("SELECT n FROM counts ORDER BY status,has_phone").unwrap().query_map([], |r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            prop_assert_eq!(before,after);
        }
    }
}

fn ids(conn: &Connection, sql: &str, term: &str) -> Vec<i64> {
    conn.prepare(sql)
        .unwrap()
        .query_map([term], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

// REQ 4.1
#[test]
fn fts_insert_delete_rebuild_matches_substrings() -> TestResult {
    let (_dir, _path, mut db) = fixture();
    db.write(0, |tx| {
        for (id, key) in [
            (1, "synthetic alpha"),
            (2, "synthetic beta"),
            (3, "gamma alpha"),
        ] {
            company(tx, id, key, 0, false);
            fts::insert(tx, id, key)?;
        }
        Ok(())
    })?;
    let check = |conn: &Connection| {
        for word in ["synthetic", "alpha", "beta", "gamma", "absent"] {
            assert_eq!(
                ids(
                    conn,
                    "SELECT rowid FROM name_fts WHERE name_fts MATCH ?1 ORDER BY rowid",
                    word
                ),
                ids(
                    conn,
                    "SELECT id FROM companies WHERE instr(name_key,?1)>0 ORDER BY id",
                    word
                )
            );
        }
    };
    check(&db.conn);
    db.write(0, |tx| {
        fts::delete(tx, 1, "synthetic alpha")?;
        tx.execute(
            "UPDATE companies SET name='gamma beta',name_key='gamma beta' WHERE id=1",
            [],
        )
        .map_err(storage)?;
        fts::insert(tx, 1, "gamma beta")?;
        fts::delete(tx, 2, "synthetic beta")?;
        tx.execute("DELETE FROM companies WHERE id=2", [])
            .map_err(storage)?;
        Ok(())
    })?;
    check(&db.conn);
    db.conn
        .execute("INSERT INTO name_fts(name_fts) VALUES('delete-all')", [])?;
    assert_eq!(
        ids(
            &db.conn,
            "SELECT rowid FROM name_fts WHERE name_fts MATCH ?1",
            "gamma"
        ),
        Vec::<i64>::new()
    );
    fts::rebuild(&db.conn)?;
    check(&db.conn);
    Ok(())
}

fn add_column(tx: &Transaction<'_>) -> Result<(), EngineError> {
    tx.execute_batch("ALTER TABLE companies ADD COLUMN synthetic_marker TEXT")
        .map_err(storage)
}
fn failing(tx: &Transaction<'_>) -> Result<(), EngineError> {
    tx.execute("UPDATE companies SET name='changed'", [])
        .map_err(storage)?;
    Err(EngineError::NotFound)
}
fn delete_row(tx: &Transaction<'_>) -> Result<(), EngineError> {
    tx.execute("DELETE FROM list_members", [])
        .map_err(storage)?;
    Ok(())
}
fn invalid_check(tx: &Transaction<'_>) -> Result<(), EngineError> {
    tx.execute_batch("CREATE TABLE invalid(x INTEGER CHECK(x>0)); PRAGMA ignore_check_constraints=ON; INSERT INTO invalid VALUES(0); PRAGMA ignore_check_constraints=OFF;").map_err(storage)
}
fn steps(second: fn(&Transaction<'_>) -> Result<(), EngineError>) -> [Migration; 2] {
    [
        Migration {
            from: 1,
            to: 2,
            apply: add_column,
        },
        Migration {
            from: 2,
            to: 3,
            apply: second,
        },
    ]
}
fn seed_all(db: &Db) {
    company(&db.conn, 1, "synthetic original", 0, false);
    db.conn.execute_batch("INSERT INTO contacts(company_id,created_at) VALUES(1,1); INSERT INTO activities(company_id,contact_id,kind,at) VALUES(1,1,4,1); INSERT INTO lists(name,created_at) VALUES('synthetic',1); INSERT INTO list_members VALUES(1,1);").unwrap();
}
fn backups(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".bak-v"))
        .collect()
}

// REQ 10.1
// REQ 10.2
#[test]
fn migration_success_has_before_backup_and_forward_version() -> TestResult {
    let (dir, path, db) = fixture();
    seed_all(&db);
    drop(db);
    migrate_with(
        &path,
        &[Migration {
            from: 1,
            to: 2,
            apply: add_column,
        }],
        None,
        &OpenOptions::default(),
    )?;
    let conn = Connection::open(&path)?;
    assert_eq!(version(&conn)?, 2);
    assert_eq!(scalar(&conn, "PRAGMA user_version"), 2);
    conn.prepare("SELECT synthetic_marker FROM companies")?;
    let copies = backups(dir.path());
    assert_eq!(copies.len(), 1);
    let copy = Db::open(&copies[0], &OpenOptions::default())?;
    assert_eq!(version(&copy.conn)?, 1);
    assert!(
        copy.conn
            .prepare("SELECT synthetic_marker FROM companies")
            .is_err()
    );
    assert_eq!(scalar(&copy.conn, "SELECT count(*) FROM list_members"), 1);
    assert!(matches!(
        Db::open(&path, &OpenOptions::default()),
        Err(EngineError::UnsupportedVersion {
            found: 2,
            supported: 1
        })
    ));
    Ok(())
}

// REQ 10.2
#[test]
fn failed_migration_rolls_back_prior_steps_and_rows() -> TestResult {
    let (dir, path, db) = fixture();
    seed_all(&db);
    drop(db);
    assert!(matches!(
        migrate_with(&path, &steps(failing), None, &OpenOptions::default()),
        Err(EngineError::NotFound)
    ));
    let db = Db::open(&path, &OpenOptions::default())?;
    assert_eq!(version(&db.conn)?, 1);
    assert_eq!(scalar(&db.conn, "PRAGMA user_version"), 1);
    let fts_sql: String = db.conn.query_row(
        "SELECT sql FROM sqlite_schema WHERE name='name_fts'",
        [],
        |r| r.get(0),
    )?;
    assert!(fts_sql.contains("tokenize='trigram case_sensitive 1'"));

    assert!(
        db.conn
            .prepare("SELECT synthetic_marker FROM companies")
            .is_err()
    );
    assert_eq!(
        db.conn
            .query_row("SELECT name FROM companies", [], |r| r.get::<_, String>(0))?,
        "synthetic original"
    );
    assert_eq!(backups(dir.path()).len(), 1);
    Ok(())
}

// REQ 10.2
// REQ 10.4
#[test]
fn post_commit_row_or_integrity_failure_restores_backup() -> TestResult {
    for bad_step in [delete_row, invalid_check] {
        let (dir, path, db) = fixture();
        seed_all(&db);
        drop(db);
        assert!(matches!(
            migrate_with(&path, &steps(bad_step), None, &OpenOptions::default()),
            Err(EngineError::Corrupt)
        ));
        let db = Db::open(
            &path,
            &OpenOptions {
                integrity: Integrity::Always,
                ..OpenOptions::default()
            },
        )?;
        assert_eq!(version(&db.conn)?, 1);
        assert_eq!(scalar(&db.conn, "PRAGMA user_version"), 1);
        let fts_sql: String = db.conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='name_fts'",
            [],
            |r| r.get(0),
        )?;
        assert!(fts_sql.contains("tokenize='trigram case_sensitive 1'"));

        for table in [
            "companies",
            "contacts",
            "activities",
            "lists",
            "list_members",
        ] {
            assert_eq!(
                scalar(&db.conn, &format!("SELECT count(*) FROM {table}")),
                1
            );
        }
        assert!(
            db.conn
                .prepare("SELECT synthetic_marker FROM companies")
                .is_err()
        );
        assert_eq!(backups(dir.path()).len(), 1);
    }
    Ok(())
}

// REQ 10.1
#[test]
fn old_new_unknown_and_unrelated_storage_are_rejected() -> TestResult {
    let (dir, path, db) = fixture();
    Db::migrate(&path, &OpenOptions::default())?;
    assert_eq!(backups(dir.path()), Vec::<PathBuf>::new());
    for (value, old) in [(0, true), (2, false)] {
        db.conn.execute(
            "UPDATE meta SET value=?1 WHERE key='schema_version'",
            [value.to_string()],
        )?;
        let error = Db::open(&path, &OpenOptions::default()).err().unwrap();
        if old {
            assert!(matches!(error, EngineError::NeedsMigration));
        } else {
            assert!(matches!(
                error,
                EngineError::UnsupportedVersion {
                    found: 2,
                    supported: 1
                }
            ));
        }
    }
    db.conn
        .execute("UPDATE meta SET value='0' WHERE key='schema_version'", [])?;
    assert!(matches!(
        Db::migrate(&path, &OpenOptions::default()),
        Err(EngineError::NeedsMigration)
    ));
    db.conn.pragma_update(None, "application_id", 0)?;
    assert!(matches!(
        Db::open(&path, &OpenOptions::default()),
        Err(EngineError::Corrupt)
    ));
    assert!(Db::open(&dir.path().join("missing.db"), &OpenOptions::default()).is_err());
    assert!(!dir.path().join("missing.db").exists());
    Ok(())
}

struct NoSpace;
impl SpaceProbe for NoSpace {
    fn available(&self, _: &Path) -> io::Result<u64> {
        Ok(0)
    }
}

// REQ 10.3
#[test]
fn no_space_prevents_migration_and_backup() -> TestResult {
    let (dir, path, db) = fixture();
    seed_all(&db);
    let opt = OpenOptions {
        space_probe: Some(Arc::new(NoSpace)),
        ..OpenOptions::default()
    };
    let db = Db::open(&path, &opt)?;
    let dest = dir.path().join("must-not-exist.db");
    assert!(matches!(
        db.backup_to(&dest),
        Err(EngineError::InsufficientSpace)
    ));
    assert!(!dest.exists());
    assert!(matches!(
        migrate_with(
            &path,
            &[Migration {
                from: 1,
                to: 2,
                apply: add_column
            }],
            Some(&NoSpace),
            &OpenOptions::default()
        ),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(backups(dir.path()), Vec::<PathBuf>::new());
    assert_eq!(version(&db.conn)?, 1);
    assert!(
        db.conn
            .prepare("SELECT synthetic_marker FROM companies")
            .is_err()
    );
    Ok(())
}

// REQ 10.2
// REQ 10.4
#[test]
fn online_backup_includes_wal_and_corrupt_middle_is_rejected() -> TestResult {
    let (dir, _path, mut db) = fixture();
    db.conn.pragma_update(None, "wal_autocheckpoint", 0)?;
    db.write(0, |tx| {
        for id in 1..=3000 {
            company(tx, id, &format!("synthetic {id:08}"), 0, false);
        }
        Ok(())
    })?;
    let dest = dir.path().join("copy.db");
    db.backup_to(&dest)?;
    let copy = Db::open(
        &dest,
        &OpenOptions {
            integrity: Integrity::Always,
            ..OpenOptions::default()
        },
    )?;
    assert_eq!(scalar(&copy.conn, "SELECT count(*) FROM companies"), 3000);
    drop(copy);
    assert!(db.backup_to(&dest).is_err());
    let mut file = fs::OpenOptions::new().write(true).open(&dest)?;
    let middle = (file.metadata()?.len() / 2 / 4096) * 4096;
    file.seek(SeekFrom::Start(middle))?;
    file.write_all(&[0xff; 4096])?;
    file.sync_all()?;
    assert!(matches!(
        Db::open(
            &dest,
            &OpenOptions {
                integrity: Integrity::Always,
                ..OpenOptions::default()
            }
        ),
        Err(EngineError::Corrupt)
    ));
    Ok(())
}

// REQ 10.4
#[test]
fn auto_checks_only_nonempty_wal() -> TestResult {
    let (_dir, path, db) = fixture();
    db.conn.pragma_update(None, "wal_autocheckpoint", 0)?;
    db.conn.execute_batch("CREATE TABLE invalid(x INTEGER CHECK(x>0)); PRAGMA ignore_check_constraints=ON; INSERT INTO invalid VALUES(0); PRAGMA ignore_check_constraints=OFF;")?;
    assert!(should_check(&path, Integrity::Auto)?);
    assert!(matches!(
        Db::open(&path, &OpenOptions::default()),
        Err(EngineError::Corrupt)
    ));
    let unchecked = Db::open(
        &path,
        &OpenOptions {
            integrity: Integrity::Never,
            ..OpenOptions::default()
        },
    )?;
    assert!(matches!(
        unchecked.verify_integrity(),
        Err(EngineError::Corrupt)
    ));
    drop(unchecked);
    db.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    assert!(!should_check(&path, Integrity::Auto)?);
    Db::open(&path, &OpenOptions::default())?;
    assert!(matches!(
        Db::open(
            &path,
            &OpenOptions {
                integrity: Integrity::Always,
                ..OpenOptions::default()
            }
        ),
        Err(EngineError::Corrupt)
    ));
    drop(db);
    assert!(!should_check(&path, Integrity::Auto)?);
    Ok(())
}

// REQ 3.1
#[test]
fn shape_two_uses_k2_search() -> TestResult {
    let (_dir, _path, mut db) = fixture();
    db.write(0, |tx| {
        for id in 1..=4000 {
            company(
                tx,
                id,
                "synthetic",
                u8::try_from(id % 6).unwrap(),
                id % 2 == 0,
            );
        }
        Ok(())
    })?;
    db.conn.execute_batch("ANALYZE")?;
    let plan:String=db.conn.query_row("EXPLAIN QUERY PLAN SELECT id FROM companies INDEXED BY k2 WHERE prefecture_code=?1 AND status=?2 ORDER BY prefecture_code,status,has_phone,id LIMIT 50",(13,0),|r|r.get(3))?;
    assert!(
        plan.starts_with("SEARCH ")
            && plan.contains("k2")
            && plan.contains("prefecture_code=? AND status=?"),
        "{plan}"
    );
    assert!(!plan.contains("TEMP B-TREE"));
    Ok(())
}

// REQ 6.2
#[test]
fn owner_change_has_keep_clear_and_set() {
    assert_ne!(OwnerChange::Keep, OwnerChange::Clear);
    assert_eq!(OwnerChange::Set(UserId(1)), OwnerChange::Set(UserId(1)));
}

// REQ 10.1
#[test]
fn oversized_sqlite_options_are_errors_instead_of_overflow_or_panic() {
    let (dir, path, _db) = fixture();
    for opt in [
        OpenOptions {
            cache_mib: u32::MAX,
            ..OpenOptions::default()
        },
        OpenOptions {
            busy_timeout_ms: u32::MAX,
            ..OpenOptions::default()
        },
    ] {
        assert!(matches!(
            Db::open(&path, &opt),
            Err(EngineError::InvalidInput { .. })
        ));
        let dest = dir.path().join("invalid-options.db");
        assert!(matches!(
            Db::create(&dest, DataOrigin::Synthetic, &opt),
            Err(EngineError::InvalidInput { .. })
        ));
        assert!(!dest.exists());
    }
}

// REQ 10.3
#[test]
fn no_space_does_not_change_journal_mode() -> TestResult {
    let (dir, path, db) = fixture();
    drop(db);
    let conn = Connection::open(&path)?;
    conn.execute_batch("PRAGMA journal_mode=DELETE")?;
    drop(conn);
    assert!(matches!(
        migrate_with(
            &path,
            &[Migration {
                from: 1,
                to: 2,
                apply: add_column
            }],
            Some(&NoSpace),
            &OpenOptions::default()
        ),
        Err(EngineError::InsufficientSpace)
    ));
    let conn = Connection::open(&path)?;
    assert_eq!(
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?,
        "delete"
    );
    assert_eq!(version(&conn)?, 1);
    assert_eq!(backups(dir.path()), Vec::<PathBuf>::new());
    Ok(())
}

// REQ 5.5, 5.8, 6.2
#[test]
fn checkpoint_and_full_sync_connection_settings() -> TestResult {
    let (_dir, path, db) = fixture();
    assert!(OpenOptions::default().background_checkpoint);
    assert_eq!(OpenOptions::default().cache_mib, 512);
    assert!(db.checkpoint.is_some());
    assert_eq!(scalar(&db.conn, "PRAGMA wal_autocheckpoint"), 0);
    assert_eq!(scalar(&db.conn, "PRAGMA fullfsync"), 0);
    assert_eq!(scalar(&db.conn, "PRAGMA checkpoint_fullfsync"), 0);
    let opt = OpenOptions {
        durability: Durability::Full,
        background_checkpoint: false,
        ..OpenOptions::default()
    };
    let full = Db::open(&path, &opt)?;
    assert!(full.checkpoint.is_none());
    assert_eq!(scalar(&full.conn, "PRAGMA wal_autocheckpoint"), 1000);
    assert_eq!(scalar(&full.conn, "PRAGMA fullfsync"), 1);
    assert_eq!(scalar(&full.conn, "PRAGMA checkpoint_fullfsync"), 1);
    assert_eq!(scalar(&full.conn, "PRAGMA synchronous"), 2);
    Ok(())
}

fn wait_for_checkpoint(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "checkpoint timeout");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn checkpoint_workload(db: &mut Db, path: &Path) -> TestResult {
    company(&db.conn, 1, "synthetic", 0, false);
    let wal = suffixed(path, "-wal");
    let note = "s".repeat(1024);
    for _ in 0..30 {
        db.write(4 * WRITE_BASE, |tx| {
            let mut insert = tx
                .prepare("INSERT INTO activities(company_id,kind,at,note) VALUES(1,4,1,?1)")
                .map_err(storage)?;
            for _ in 0..1000 {
                insert.execute([&note]).map_err(storage)?;
            }
            Ok(())
        })?;
        let (log, done) = wal_state(db);
        if db.checkpoint.is_some() && log - done >= 1000 {
            wait_for_checkpoint(|| {
                let (log, done) = wal_state(db);
                log == done
            });
        }
        assert!(fs::metadata(&wal)?.len() <= checkpoint::WAL_THRESHOLD * 4);
    }
    db.settle()?;
    assert_eq!(scalar(&db.conn, "SELECT count(*) FROM activities"), 30_000);
    assert!(fs::metadata(path)?.len() > checkpoint::WAL_THRESHOLD * 8);
    Ok(())
}

// REQ 5.5, 6.2
#[test]
fn background_checkpoint_bounds_wal_and_flushes_committed_pages() -> TestResult {
    let (_dir, path, mut db) = fixture();
    checkpoint_workload(&mut db, &path)?;
    assert!(db.checkpoint.is_some());
    Ok(())
}

// REQ 5.5, 6.2
#[test]
fn foreground_checkpoint_preserves_legacy_wal_bound_without_worker() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("foreground.db");
    let opt = OpenOptions {
        background_checkpoint: false,
        ..OpenOptions::default()
    };
    let mut db = Db::create(&path, DataOrigin::Synthetic, &opt)?;
    assert!(db.checkpoint.is_none());
    assert_eq!(scalar(&db.conn, "PRAGMA wal_autocheckpoint"), 1000);
    checkpoint_workload(&mut db, &path)?;
    Ok(())
}

// REQ 5.5, 6.2
#[test]
fn sqlite_cache_limit_accepts_the_boundary_and_rejects_the_next_value() -> TestResult {
    let (_dir, path, _db) = fixture();
    let limit = i32::MAX.unsigned_abs() / 1024;
    let mut opt = OpenOptions {
        cache_mib: limit,
        ..OpenOptions::default()
    };
    let db = Db::open(&path, &opt)?;
    assert_eq!(
        scalar(&db.conn, "PRAGMA cache_size"),
        -(i64::from(limit) * 1024)
    );
    opt.cache_mib += 1;
    assert!(
        matches!(Db::open(&path, &opt), Err(EngineError::InvalidInput { field, .. }) if field == "cache_mib")
    );
    Ok(())
}

fn wal_state(db: &Db) -> (i64, i64) {
    db.conn()
        .query_row("PRAGMA wal_checkpoint(NOOP)", [], |r| {
            Ok((r.get(1)?, r.get(2)?))
        })
        .unwrap()
}

// REQ 5.7, 5.8, 6.2
#[test]
fn backpressure_bounds_uninterrupted_activity_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let opt = OpenOptions {
        wal_limit_mib: 8,
        ..OpenOptions::default()
    };
    let mut db = Db::create(&dir.path().join("pressure.db"), DataOrigin::Synthetic, &opt)?;
    company(db.conn(), 1, "synthetic", 0, false);
    let page_size = scalar(db.conn(), "PRAGMA page_size");
    let limit = 8 * 1024 * 1024 / page_size;
    let mut maximum = 0;
    let note = "s".repeat(1024);
    for _ in 0..30_000 {
        let (log, _) = wal_state(&db);
        maximum = maximum.max(log);
        assert!(log <= limit + 300, "WAL log={log}, limit={limit}");
        db.write(WRITE_BASE, |tx| {
            tx.execute(
                "INSERT INTO activities(company_id,kind,at,note) VALUES(1,4,1,?1)",
                [&note],
            )
            .map_err(storage)?;
            Ok(())
        })?;
    }
    eprintln!("backpressure: max log={maximum}, limit={limit}");
    assert_eq!(scalar(db.conn(), "SELECT count(*) FROM activities"), 30_000);
    Ok(())
}

// REQ 5.5
// REQ 5.6
#[test]
fn backpressure_skips_light_activity_with_pinned_reader_above_limit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("light-activity.db");
    let opt = OpenOptions {
        wal_limit_mib: 64,
        ..Default::default()
    };
    assert_eq!(opt.busy_timeout_ms, 5000);
    let mut db = Db::create(&path, DataOrigin::Synthetic, &opt)?;
    company(db.conn(), 1, "synthetic", 0, false);
    assert!(db.settle()?);
    let reader = Connection::open(&path)?;
    reader.execute_batch("BEGIN; SELECT * FROM companies")?;
    db.write(16 * WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('large',zeroblob(73400320))", [])
            .map_err(storage)?;
        Ok(())
    })?;
    assert!(wal_state(&db).0 * scalar(db.conn(), "PRAGMA page_size") > 64 * 1024 * 1024);
    let mut maximum = Duration::ZERO;
    for at in 1..=30 {
        let start = std::time::Instant::now();
        crate::write::record_activity(
            &mut db,
            &crate::types::ActivityInput {
                company: crate::types::CompanyId(1),
                contact: None,
                kind: crate::types::ActivityKind::Call,
                at,
                note: None,
                status_after: None,
                next_action: crate::types::NextAction::Keep,
                reverses: None,
            },
        )?;
        maximum = maximum.max(start.elapsed());
    }
    assert!(maximum < Duration::from_secs(1), "{maximum:?}");
    eprintln!("light activity above WAL limit: 30 writes, max={maximum:?}");
    assert_eq!(scalar(db.conn(), "SELECT count(*) FROM activities"), 30);
    assert_eq!(scalar(db.conn(), "PRAGMA busy_timeout"), 5000);
    reader.execute_batch("ROLLBACK")?;
    Ok(())
}

// REQ 5.7, 5.8, 6.2
#[test]
fn backpressure_times_out_with_pinned_reader_and_preserves_timeout() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("reader.db");
    let opt = OpenOptions {
        wal_limit_mib: 8,
        busy_timeout_ms: 50,
        ..OpenOptions::default()
    };
    let mut db = Db::create(&path, DataOrigin::Synthetic, &opt)?;
    db.settle()?;
    let reader = Connection::open(&path)?;
    reader.execute_batch("BEGIN; SELECT * FROM meta;")?;
    db.write(16 * WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('large',zeroblob(10000000))", [])
            .map_err(storage)?;
        Ok(())
    })?;
    assert!(wal_state(&db).0 >= 2048);
    let start = std::time::Instant::now();
    db.write(WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('after','success')", [])
            .map_err(storage)?;
        Ok(())
    })?;
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(35), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_eq!(scalar(db.conn(), "PRAGMA busy_timeout"), 50);
    let start = std::time::Instant::now();
    assert!(!db.settle()?);
    assert!(start.elapsed() >= Duration::from_millis(35));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(scalar(db.conn(), "PRAGMA busy_timeout"), 50);
    reader.execute_batch("ROLLBACK")?;
    assert!(db.settle()?);
    let (log, done) = wal_state(&db);
    assert_eq!(log, done);
    Ok(())
}

// REQ 5.8, 6.2
#[test]
fn checkpoint_wakes_for_pending_frames_instead_of_file_high_water() -> TestResult {
    let (_dir, path, mut db) = fixture();
    db.settle()?;
    db.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    db.write(4 * WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('small',zeroblob(3000000))", [])
            .map_err(storage)?;
        Ok(())
    })?;
    let before = wal_state(&db);
    assert!(before.0 < 1000);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(wal_state(&db), before);
    db.write(4 * WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('large',zeroblob(2000000))", [])
            .map_err(storage)?;
        Ok(())
    })?;
    assert!(wal_state(&db).0 >= 1000);
    wait_for_checkpoint(|| {
        let (log, done) = wal_state(&db);
        log == done
    });
    assert!(fs::metadata(suffixed(&path, "-wal"))?.len() > checkpoint::WAL_THRESHOLD);
    std::thread::sleep(Duration::from_millis(100));
    db.write(WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('tiny','value')", [])
            .map_err(storage)?;
        Ok(())
    })?;
    let before = wal_state(&db);
    assert!(before.0 - before.1 < 1000);
    assert!(before.0 > before.1);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(wal_state(&db), before);
    Ok(())
}

// REQ 5.8, 6.2
#[test]
fn foreground_mode_skips_backpressure_and_settle() -> TestResult {
    let dir = tempfile::tempdir()?;
    let opt = OpenOptions {
        background_checkpoint: false,
        wal_limit_mib: 8,
        ..OpenOptions::default()
    };
    let mut db = Db::create(
        &dir.path().join("foreground.db"),
        DataOrigin::Synthetic,
        &opt,
    )?;
    assert!(db.checkpoint.is_none());
    assert_eq!(scalar(db.conn(), "PRAGMA wal_autocheckpoint"), 1000);
    // 自動書き戻しも止め、背景方式が無効なら上限を越えても介入しないことを確かめる。
    db.conn().pragma_update(None, "wal_autocheckpoint", 0)?;
    db.write(16 * WRITE_BASE, |tx| {
        tx.execute("INSERT INTO meta VALUES('large',zeroblob(10000000))", [])
            .map_err(storage)?;
        Ok(())
    })?;
    let before = wal_state(&db);
    assert!(before.0 > 2048 && before.0 > before.1);
    assert!(db.settle()?);
    assert_eq!(wal_state(&db), before);
    db.write(0, |_| Ok(()))?;
    assert_eq!(wal_state(&db), before);
    Ok(())
}

// REQ 5.8, 6.2
#[test]
fn wal_limit_validates_minimum_and_accepts_u32_max() -> TestResult {
    let (_dir, path, _db) = fixture();
    assert_eq!(OpenOptions::default().wal_limit_mib, 512);
    for value in [0, 7] {
        let opt = OpenOptions {
            wal_limit_mib: value,
            ..OpenOptions::default()
        };
        assert!(
            matches!(Db::open(&path, &opt), Err(EngineError::InvalidInput { field, .. }) if field == "wal_limit_mib")
        );
    }
    for value in [8, u32::MAX] {
        let opt = OpenOptions {
            wal_limit_mib: value,
            ..OpenOptions::default()
        };
        let mut db = Db::open(&path, &opt)?;
        db.write(0, |_| Ok(()))?;
    }
    Ok(())
}

// REQ 10.3
#[test]
fn create_checks_space_before_creating_file() -> TestResult {
    struct NoSpace;
    impl SpaceProbe for NoSpace {
        fn available(&self, _: &Path) -> io::Result<u64> {
            Ok(0)
        }
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("no-space.db");
    let opt = OpenOptions {
        space_probe: Some(Arc::new(NoSpace)),
        ..Default::default()
    };
    assert!(matches!(
        Db::create(&path, DataOrigin::Synthetic, &opt),
        Err(EngineError::InsufficientSpace)
    ));
    assert!(!path.exists());
    Ok(())
}

// REQ 5.8
#[test]
fn retry_zero_timeout_and_error_restore_busy_timeout() -> TestResult {
    let (_dir, path, db) = fixture();
    assert!(db.settle()?);
    let reader = Connection::open(&path)?;
    reader.execute_batch("BEGIN; SELECT * FROM meta")?;
    db.conn()
        .execute("INSERT INTO meta VALUES('retry','value')", [])?;
    db.conn().busy_timeout(Duration::ZERO)?;
    let start = std::time::Instant::now();
    assert!(!checkpoint::retry(db.conn(), checkpoint::Mode::Restart)?);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert_eq!(scalar(db.conn(), "PRAGMA busy_timeout"), 0);
    reader.execute_batch("ROLLBACK")?;
    db.conn().busy_timeout(Duration::from_millis(123))?;
    // 自接続の書き込み中はチェックポイントがSQLITE_LOCKEDで失敗する。
    db.conn()
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO meta VALUES('locked','value')")?;
    assert!(checkpoint::retry(db.conn(), checkpoint::Mode::Restart).is_err());
    assert_eq!(scalar(db.conn(), "PRAGMA busy_timeout"), 123);
    db.conn().execute_batch("ROLLBACK")?;
    Ok(())
}
