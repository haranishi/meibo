use super::support::*;
use meibo_core::{Engine, Integrity, OpenOptions, SpaceProbe, synth::Profile, types::*};
use std::{
    io::{Seek, SeekFrom, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

// REQ 10.1
// REQ 10.4
#[test]
fn backups_corruption_and_future_versions() {
    let (dir, mut db) = database();
    db.load_synthetic(config(1000, Profile::Dense)).unwrap();
    let backup = dir.path().join("backup.db");
    db.backup_to(&backup).unwrap();
    let copy = Engine::open(&backup, &OpenOptions::default()).unwrap();
    assert_eq!(
        all(&db, query(Filters::default(), Sort::ShapeDefault)),
        all(&copy, query(Filters::default(), Sort::ShapeDefault))
    );
    verify(&copy);
    drop(copy);
    Engine::migrate(&backup, &OpenOptions::default()).unwrap();
    let future = dir.path().join("future.db");
    db.backup_to(&future).unwrap();
    let conn = rusqlite::Connection::open(&future).unwrap();
    conn.execute("UPDATE meta SET value='999' WHERE key='schema_version'", [])
        .unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    assert!(matches!(
        Engine::open(&future, &OpenOptions::default()),
        Err(EngineError::UnsupportedVersion { found: 999, .. })
    ));
    let corrupt = dir.path().join("corrupt.db");
    db.backup_to(&corrupt).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&corrupt)
        .unwrap();
    let size = file.metadata().unwrap().len();
    file.seek(SeekFrom::Start((size / 2 / 4096) * 4096))
        .unwrap();
    file.write_all(&[0xff; 4096]).unwrap();
    drop(file);
    let opt = OpenOptions {
        integrity: Integrity::Always,
        ..OpenOptions::default()
    };
    match Engine::open(&corrupt, &opt) {
        Err(EngineError::Corrupt) => assert!(
            EngineError::Corrupt
                .to_string()
                .contains("restore from a verified backup")
        ),
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("opened corrupt database"),
    }
}

struct FakeSpace(AtomicU64);
impl SpaceProbe for FakeSpace {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
// REQ 10.3
#[test]
fn injected_space_protects_bulk_and_double_size_compact() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(FakeSpace(AtomicU64::new(u64::MAX)));
    let opt = OpenOptions {
        space_probe: Some(probe.clone()),
        ..OpenOptions::default()
    };
    let mut db = Engine::create(&dir.path().join("space.db"), DataOrigin::Synthetic, &opt).unwrap();
    let id = db.register_company(&company()).unwrap();
    db.compact().unwrap();
    let before = db.company_detail(id, &fields()).unwrap();
    let bytes = std::fs::read(db.path().unwrap()).unwrap();
    for available in [0, bytes.len() as u64 + bytes.len() as u64 / 2] {
        probe.0.store(available, Ordering::SeqCst);
        assert!(matches!(
            db.load_synthetic(config(1000, Profile::Dense)),
            Err(EngineError::InsufficientSpace)
        ));
        assert!(matches!(db.compact(), Err(EngineError::InsufficientSpace)));
        assert_eq!(std::fs::read(db.path().unwrap()).unwrap(), bytes);
        assert_eq!(db.company_detail(id, &fields()).unwrap(), before);
        assert_eq!(db.count(&Filters::default()).unwrap().n, 1);
        verify(&db);
    }
    probe.0.store(u64::MAX, Ordering::SeqCst);
    db.load_synthetic(config(1000, Profile::Dense)).unwrap();
    db.compact().unwrap();
    verify(&db);
}

fn contents(db: &Engine) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = rusqlite::Connection::open_with_flags(
        db.path().unwrap(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    [
        "companies",
        "contacts",
        "activities",
        "lists",
        "list_members",
        "counts",
        "erase_gate",
    ]
    .iter()
    .map(|table| {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
            .unwrap();
        let columns = stmt.column_count();
        stmt.query_map([], |r| {
            (0..columns)
                .map(|i| r.get(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    })
    .collect()
}

// REQ 10.3
#[test]
fn every_write_checks_space_and_preserves_contents() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(FakeSpace(AtomicU64::new(u64::MAX)));
    let opt = OpenOptions {
        space_probe: Some(probe.clone()),
        ..Default::default()
    };
    let mut db = Engine::create(
        &dir.path().join("all-writes.db"),
        DataOrigin::Synthetic,
        &opt,
    )
    .unwrap();
    let mut c = company();
    c.is_sole_proprietor = true;
    let id = db.register_company(&c).unwrap();
    let mut excluded = c.clone();
    excluded.status = Status::Excluded;
    let excluded_id = db.register_company(&excluded).unwrap();
    let contact = ContactInput {
        company: id,
        name: "synthetic contact".into(),
        phone: Some("0123".into()),
        email: Some("synthetic@example.invalid".into()),
    };
    let contact_id = db.add_contact(&contact).unwrap();
    let mut a = activity(id);
    a.contact = Some(contact_id);
    db.record_activity(&a).unwrap();
    let list = db
        .create_list("synthetic list", &Filters::default(), 100)
        .unwrap();
    let change = BulkChange {
        owner: OwnerChange::Set(UserId(8)),
        status: Some(Status::Won),
        include_excluded: false,
    };
    let before = contents(&db);
    let details = [
        db.company_detail(id, &fields()).unwrap(),
        db.company_detail(excluded_id, &fields()).unwrap(),
    ];
    probe.0.store(0, Ordering::SeqCst);
    for operation in 0..10 {
        let result = match operation {
            0 => db.register_company(&c).map(|_| ()),
            1 => db.update_company(id, &c),
            2 => db.add_contact(&contact).map(|_| ()),
            3 => db.record_activity(&a).map(|_| ()),
            4 => db
                .release_exclusion(excluded_id, Status::Untouched)
                .map(|_| ()),
            5 => db
                .create_list("new list", &Filters::default(), 100)
                .map(|_| ()),
            6 => db.delete_list(list),
            7 => db.bulk_update(list, &change).map(|_| ()),
            8 => db.erase_contact(contact_id, true),
            _ => db.erase_company(id),
        };
        assert!(
            matches!(result, Err(EngineError::InsufficientSpace)),
            "operation {operation}: {result:?}"
        );
        assert_eq!(contents(&db), before);
        assert_eq!(db.company_detail(id, &fields()).unwrap(), details[0]);
        assert_eq!(
            db.company_detail(excluded_id, &fields()).unwrap(),
            details[1]
        );
        assert_eq!(
            db.count(&Filters {
                include_excluded: true,
                ..Default::default()
            })
            .unwrap()
            .n,
            2
        );
        assert_eq!(
            db.list(&query(Filters::default(), Sort::ShapeDefault))
                .unwrap()
                .rows
                .len(),
            1
        );
        verify(&db);
    }
    probe.0.store(u64::MAX, Ordering::SeqCst);
    db.register_company(&c).unwrap();
    db.update_company(id, &c).unwrap();
    db.add_contact(&contact).unwrap();
    db.record_activity(&a).unwrap();
    db.release_exclusion(excluded_id, Status::Untouched)
        .unwrap();
    let other = db.create_list("other", &Filters::default(), 100).unwrap();
    db.delete_list(other).unwrap();
    assert_eq!(db.bulk_update(list, &change).unwrap(), 1);
    db.erase_contact(contact_id, true).unwrap();
    db.erase_company(id).unwrap();
    verify(&db);
}

// REQ 10.3
#[test]
fn large_strings_increase_write_estimates() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(FakeSpace(AtomicU64::new(u64::MAX)));
    let opt = OpenOptions {
        space_probe: Some(probe.clone()),
        ..Default::default()
    };
    let mut db = Engine::create(&dir.path().join("large.db"), DataOrigin::Synthetic, &opt).unwrap();
    let id = db.register_company(&company()).unwrap();
    let before = contents(&db);
    probe.0.store(20 * 1024 * 1024, Ordering::SeqCst);
    let large = "x".repeat(10 * 1024 * 1024);
    let mut a = activity(id);
    a.note = Some(large.clone());
    let mut c = company();
    c.website = Some(large.clone());
    let contact = ContactInput {
        company: id,
        name: large,
        phone: None,
        email: None,
    };
    assert!(matches!(
        db.record_activity(&a),
        Err(EngineError::InsufficientSpace)
    ));
    assert!(matches!(
        db.register_company(&c),
        Err(EngineError::InsufficientSpace)
    ));
    assert!(matches!(
        db.update_company(id, &c),
        Err(EngineError::InsufficientSpace)
    ));
    assert!(matches!(
        db.add_contact(&contact),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(contents(&db), before);
    verify(&db);
}

// REQ 8.1
#[test]
fn synthetic_rows_over_limit_are_invalid_before_space_probe() {
    struct UnexpectedProbe;
    impl SpaceProbe for UnexpectedProbe {
        fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
            panic!("invalid rows must not probe space")
        }
    }
    let (dir, db) = database();
    let path = db.path().unwrap().to_owned();
    let before = contents(&db);
    drop(db);
    let mut db = Engine::open(
        &path,
        &OpenOptions {
            space_probe: Some(Arc::new(UnexpectedProbe)),
            ..Default::default()
        },
    )
    .unwrap();
    for rows in [300_000_000_001, u64::MAX] {
        assert!(
            matches!(db.load_synthetic(config(rows, Profile::Dense)), Err(EngineError::InvalidInput {field, ..}) if field == "rows")
        );
        assert_eq!(contents(&db), before);
        verify(&db);
    }
    drop(dir);
}

// REQ 5.4
#[test]
fn verify_uses_one_snapshot_during_concurrent_activity_writes() {
    use std::sync::atomic::AtomicBool;
    let (_dir, mut db) = database();
    for _ in 0..100 {
        db.register_company(&company()).unwrap();
    }
    let path = db.path().unwrap().to_owned();
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let writer_stop = stop.clone();
    let writer_written = written.clone();
    let (ready, started) = std::sync::mpsc::sync_channel(0);
    let writer = std::thread::spawn(move || {
        let mut db = Engine::open(&path, &OpenOptions::default()).unwrap();
        let mut n = 0;
        loop {
            let mut a = activity(CompanyId(n % 100 + 1));
            a.at = n;
            a.status_after = Some(if n % 2 == 0 {
                Status::Won
            } else {
                Status::InProgress
            });
            db.record_activity(&a).unwrap();
            n += 1;
            writer_written.fetch_add(1, Ordering::SeqCst);
            if n == 1 {
                ready.send(()).unwrap();
            }
            if writer_stop.load(Ordering::SeqCst) {
                break;
            }
        }
    });
    started.recv().unwrap();
    let before = written.load(Ordering::SeqCst);
    let reports: Vec<_> = (0..50).map(|_| db.verify()).collect();
    let after = written.load(Ordering::SeqCst);
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    assert!(after > before, "writer must progress during verification");
    for report in reports {
        let r = report.unwrap();
        assert!(r.integrity_ok && r.counts_match && r.folds_match, "{r:?}");
    }
}

// REQ 5.7, 5.8
#[test]
fn background_checkpoint_drop_joins_and_reopens_with_consistent_data() {
    let (_dir, mut db) = database();
    db.load_synthetic(config(1000, Profile::Dense)).unwrap();
    let path = db.path().unwrap().to_owned();
    let id = CompanyId(1);
    let mut a = activity(id);
    a.note = Some("synthetic memo".repeat(1024));
    for _ in 0..400 {
        db.record_activity(&a).unwrap();
    }
    let (done, finished) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(db);
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("Engine drop timeout");
    dropper.join().unwrap();
    let db = Engine::open(&path, &OpenOptions::default()).unwrap();
    verify(&db);
}

// REQ 5.6, 5.7, 6.2
#[test]
fn background_checkpoint_allows_reads_compact_and_backup_during_writes() {
    let (dir, mut db) = database();
    let id = db.register_company(&company()).unwrap();
    let path = db.path().unwrap().to_owned();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_stop = stop.clone();
    let (ready, started) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let conn =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let mut iterations = 0;
        loop {
            let n: i64 = conn
                .query_row("SELECT count(*) FROM companies", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1);
            iterations += 1;
            if iterations == 1 {
                ready.send(()).unwrap();
            }
            if reader_stop.load(Ordering::Acquire) {
                return iterations;
            }
        }
    });
    started
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let mut a = activity(id);
    a.note = Some("synthetic memo".repeat(1024));
    let writes = (|| {
        for _ in 0..1000 {
            db.record_activity(&a)?;
        }
        db.compact()?;
        db.backup_to(&dir.path().join("checkpoint-backup.db"))
    })();
    stop.store(true, Ordering::Release);
    assert!(reader.join().unwrap() > 1);
    writes.unwrap();
    verify(&db);
    let backup = Engine::open(
        &dir.path().join("checkpoint-backup.db"),
        &OpenOptions::default(),
    )
    .unwrap();
    verify(&backup);
}

// REQ 5.7, 5.8, 6.2
#[test]
fn settle_finishes_background_checkpoint_without_truncating_wal() {
    let (_dir, mut db) = database();
    let id = db.register_company(&company()).unwrap();
    let mut a = activity(id);
    a.note = Some("synthetic".repeat(100_000));
    for _ in 0..10 {
        db.record_activity(&a).unwrap();
    }
    assert!(db.settle().unwrap());
    let conn = rusqlite::Connection::open(db.path().unwrap()).unwrap();
    let (log, done): (i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(NOOP)", [], |r| {
            Ok((r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert!(log > 0);
    assert_eq!(log, done);
    let mut wal = db.path().unwrap().as_os_str().to_os_string();
    wal.push("-wal");
    assert!(std::fs::metadata(std::path::Path::new(&wal)).unwrap().len() > 0);
    verify(&db);
}
