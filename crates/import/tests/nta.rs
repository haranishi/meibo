//! Public catalog boundary and partial-commit integration checks.
use meibo_core::{
    Engine, OpenOptions,
    types::{DataOrigin, Filters, ListQuery},
};
use meibo_import::sync_nta;
use rusqlite::{Connection, params};

fn catalog(path: &std::path::Path, count: u64) {
    let mut db = Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE source_batches(id INTEGER PRIMARY KEY,source_type TEXT,state TEXT,signature_verified INTEGER,signature_fingerprint TEXT,as_of TEXT);CREATE TABLE companies(corporate_number TEXT PRIMARY KEY,legal_name TEXT,prefecture_code TEXT,source_batch_id INTEGER,phone TEXT,industry TEXT);INSERT INTO source_batches VALUES(1,'nta_monthly_all','complete',1,'fixture','2026-10-23')").unwrap();
    let tx = db.transaction().unwrap();
    for n in 0..count {
        tx.execute("INSERT INTO companies VALUES(?1,'合成企業','13',1,'取り込んではいけない値','不許可出典')",[meibo_core::houjin::generate(n).unwrap()]).unwrap();
    }
    tx.commit().unwrap();
}

#[test]
fn imports_only_allowed_fields_and_never_changes_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("catalog.db");
    catalog(&src, 2);
    let before = std::fs::read(&src).unwrap();
    let mut crm = Engine::create(
        &dir.path().join("crm.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )
    .unwrap();
    assert_eq!(sync_nta(&mut crm, &src, None).unwrap().inserted, 2);
    assert_eq!(sync_nta(&mut crm, &src, None).unwrap().updated, 2);
    let page = crm
        .list(&ListQuery {
            filters: Filters::default(),
            sort: meibo_core::types::Sort::ShapeDefault,
            after: None,
            limit: 20,
            fields: vec![],
        })
        .unwrap();
    assert!(page.rows.iter().all(|r| r.phone.is_none()
        && r.industry.is_none()
        && r.source_id.as_deref() == Some("nta")));
    assert_eq!(std::fs::read(&src).unwrap(), before);
    assert!(crm.verify().unwrap().integrity_ok);
}

#[test]
fn unverified_or_different_sources_are_rejected_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("catalog.db");
    catalog(&src, 1);
    let mut crm = Engine::create(
        &dir.path().join("crm.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )
    .unwrap();
    let conn = Connection::open(&src).unwrap();
    conn.execute("UPDATE source_batches SET source_type='shokubarabo'", [])
        .unwrap();
    assert!(sync_nta(&mut crm, &src, None).is_err());
    conn.execute(
        "UPDATE source_batches SET source_type='nta_monthly_all',signature_verified=0",
        [],
    )
    .unwrap();
    assert!(sync_nta(&mut crm, &src, None).is_err());
    assert_eq!(crm.count(&Filters::default()).unwrap().n, 0);
    conn.execute(
        "UPDATE source_batches SET state=NULL,signature_verified=NULL,signature_fingerprint=NULL",
        [],
    )
    .unwrap();
    assert!(sync_nta(&mut crm, &src, None).is_err());
    assert_eq!(crm.count(&Filters::default()).unwrap().n, 0);
}

#[test]
fn daily_overlay_applies_additions_updates_and_removals() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("catalog.db");
    catalog(&src, 2);
    let db = Connection::open(&src).unwrap();
    db.execute_batch("CREATE TABLE daily_parts(as_of TEXT,part_number INTEGER,signature_fingerprint TEXT,csv_sha256 TEXT);CREATE TABLE daily_changes(corporate_number TEXT PRIMARY KEY,legal_name TEXT,prefecture_code TEXT,is_deleted INTEGER,as_of TEXT,part_number INTEGER);INSERT INTO daily_parts VALUES('2026-10-24',1,'fixture','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')").unwrap();
    db.execute(
        "INSERT INTO daily_changes VALUES(?1,NULL,NULL,1,'2026-10-24',1)",
        [meibo_core::houjin::generate(0).unwrap()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO daily_changes VALUES(?1,'合成変更','27',0,'2026-10-24',1)",
        [meibo_core::houjin::generate(1).unwrap()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO daily_changes VALUES(?1,'合成追加','13',0,'2026-10-24',1)",
        [meibo_core::houjin::generate(2).unwrap()],
    )
    .unwrap();
    let mut crm = Engine::create(
        &dir.path().join("crm.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )
    .unwrap();
    assert_eq!(sync_nta(&mut crm, &src, None).unwrap().inserted, 2);
    let rows = crm
        .list(&ListQuery {
            filters: Filters::default(),
            sort: meibo_core::types::Sort::ShapeDefault,
            after: None,
            limit: 20,
            fields: vec![],
        })
        .unwrap()
        .rows;
    assert!(
        rows.iter()
            .any(|r| r.name.as_deref() == Some("合成変更") && r.prefecture == 27)
    );
    assert!(rows.iter().any(|r| r.name.as_deref() == Some("合成追加")));
    assert!(
        rows.iter()
            .all(|r| r.corporate_number.as_deref()
                != Some(&meibo_core::houjin::generate(0).unwrap()))
    );
}

#[test]
fn later_failure_reports_only_previously_committed_batches() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("catalog.db");
    catalog(&src, 500);
    Connection::open(&src)
        .unwrap()
        .execute(
            "INSERT INTO companies VALUES(?1,'bad','99',1,NULL,NULL)",
            params!["z-invalid"],
        )
        .unwrap();
    let mut crm = Engine::create(
        &dir.path().join("crm.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )
    .unwrap();
    let error = sync_nta(&mut crm, &src, None).unwrap_err();
    assert_eq!(error.row, 501);
    assert_eq!(error.report.inserted, 500);
    assert_eq!(crm.count(&Filters::default()).unwrap().n, 500);
    assert!(!error.to_string().contains("z-invalid"));
}
