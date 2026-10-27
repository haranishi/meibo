use super::support::*;
use meibo_core::{
    DuplicatePolicy, Engine, ImportBatchReport, OpenOptions, SpaceProbe, houjin, types::*,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn numbered(sequence: u64) -> CompanyInput {
    CompanyInput {
        corporate_number: Some(houjin::generate(sequence).unwrap()),
        ..company()
    }
}

// REQ 12.1
// REQ 12.2
#[test]
fn imported_companies_share_validation_search_counts_and_initial_status() {
    let (_dir, mut db) = database();
    let input = CompanyInput {
        name: "ＡＢＣかな".into(),
        phone: Some(" ０１２３ ".into()),
        status: Status::Negotiating,
        ..numbered(0)
    };
    assert_eq!(
        db.import_companies(&[input], DuplicatePolicy::Error)
            .unwrap(),
        ImportBatchReport {
            inserted: 1,
            skipped: 0
        }
    );
    let result = db.search_name("abcカナ", &search()).unwrap();
    assert_eq!(result.n, Some(1));
    assert_eq!(result.rows[0].phone.as_deref(), Some("0123"));
    assert_eq!(result.rows[0].name.as_deref(), Some("ＡＢＣかな"));
    assert_eq!(result.rows[0].status, Status::Negotiating);
    let detail = db.company_detail(result.rows[0].id, &fields()).unwrap();
    assert!(detail.activities.is_empty() && detail.contacts.is_empty());
    assert_eq!(
        db.count(&Filters {
            status: vec![Status::Negotiating],
            has_phone: Some(true),
            ..Filters::default()
        })
        .unwrap()
        .n,
        1
    );
    verify(&db);
}

// REQ 12.2
// REQ 12.3
#[test]
fn duplicate_policy_covers_existing_and_batch_rows_without_updating_them() {
    let (_dir, mut db) = database();
    db.import_companies(&[numbered(0)], DuplicatePolicy::Error)
        .unwrap();
    let id = db
        .list(&query(Filters::default(), Sort::ShapeDefault))
        .unwrap()
        .rows[0]
        .id;
    db.record_activity(&activity(id)).unwrap();
    let before = db.company_detail(id, &fields()).unwrap();
    let changed = CompanyInput {
        name: "書き換えてはいけない企業".into(),
        status: Status::Won,
        ..numbered(0)
    };
    let new = CompanyInput {
        name: "追加用合成会社".into(),
        ..numbered(1)
    };
    let error = db
        .import_companies(&[new.clone(), changed.clone()], DuplicatePolicy::Error)
        .unwrap_err();
    assert_eq!(error.row, 2);
    assert!(
        matches!(error.error, EngineError::DuplicateCorporateNumber { existing } if existing == id)
    );
    let error = db
        .import_companies(&[new.clone(), new.clone()], DuplicatePolicy::Error)
        .unwrap_err();
    assert_eq!(error.row, 2);
    assert!(matches!(
        error.error,
        EngineError::DuplicateCorporateNumber { .. }
    ));
    assert_eq!(db.search_name("追加用", &search()).unwrap().n, Some(0));
    assert_eq!(db.count(&Filters::default()).unwrap().n, 1);
    assert_eq!(
        db.import_companies(
            &[changed, new.clone(), new, company(), company()],
            DuplicatePolicy::Skip
        )
        .unwrap(),
        ImportBatchReport {
            inserted: 3,
            skipped: 2
        }
    );
    assert_eq!(db.company_detail(id, &fields()).unwrap(), before);
    assert_eq!(db.search_name("追加用", &search()).unwrap().n, Some(1));
    assert_eq!(db.count(&Filters::default()).unwrap().n, 4);
    verify(&db);
}

// REQ 12.1
// REQ 12.2
// REQ 12.3
#[test]
fn invalid_rows_and_invalid_duplicates_roll_back_the_whole_batch() {
    let (_dir, mut db) = database();
    let initial = db.register_company(&numbered(0)).unwrap();
    let before = db.company_detail(initial, &fields()).unwrap();
    let good = CompanyInput {
        name: "巻戻し確認会社".into(),
        ..numbered(1)
    };
    let invalid = [
        CompanyInput {
            origin: DataOrigin::Real,
            ..numbered(2)
        },
        CompanyInput {
            name: " ".into(),
            ..numbered(2)
        },
        CompanyInput {
            corporate_number: Some("123".into()),
            ..company()
        },
        CompanyInput {
            prefecture: 48,
            ..numbered(2)
        },
        CompanyInput {
            municipality: Some("14101".into()),
            ..numbered(2)
        },
        CompanyInput {
            source_as_of: Some("2025-02-29".into()),
            ..numbered(2)
        },
        CompanyInput {
            source_id: Some("bad\0source".into()),
            ..numbered(2)
        },
    ];
    for input in invalid {
        let error = db
            .import_companies(&[good.clone(), input], DuplicatePolicy::Error)
            .unwrap_err();
        assert_eq!(error.row, 2);
        assert_eq!(db.count(&Filters::default()).unwrap().n, 1);
        assert_eq!(db.search_name("巻戻し", &search()).unwrap().n, Some(0));
        assert_eq!(db.company_detail(initial, &fields()).unwrap(), before);
        verify(&db);
    }
    for input in [
        CompanyInput {
            origin: DataOrigin::Real,
            ..numbered(0)
        },
        CompanyInput {
            prefecture: 48,
            ..numbered(0)
        },
    ] {
        let error = db
            .import_companies(&[input], DuplicatePolicy::Skip)
            .unwrap_err();
        assert_eq!(error.row, 1);
        assert!(!matches!(
            error.error,
            EngineError::DuplicateCorporateNumber { .. }
        ));
    }
    assert_eq!(db.company_detail(initial, &fields()).unwrap(), before);
    verify(&db);
}

// REQ 12.2
#[test]
fn failure_after_fulltext_insert_rolls_back_rows_index_and_counts() {
    let (_dir, mut db) = database();
    db.import_companies(&[numbered(0)], DuplicatePolicy::Error)
        .unwrap();
    let conn = crate::sql::Connection::open(db.path().unwrap()).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_import BEFORE UPDATE ON counts WHEN NEW.status=3 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let error = db
        .import_companies(
            &[
                CompanyInput {
                    name: "索引巻戻し会社".into(),
                    ..numbered(1)
                },
                CompanyInput {
                    name: "索引巻戻し会社".into(),
                    status: Status::Won,
                    ..numbered(2)
                },
            ],
            DuplicatePolicy::Error,
        )
        .unwrap_err();
    assert_eq!(error.row, 2);
    assert!(matches!(error.error, EngineError::Storage(_)));
    assert_eq!(db.count(&Filters::default()).unwrap().n, 1);
    assert_eq!(db.search_name("索引巻戻し", &search()).unwrap().n, Some(0));
    verify(&db);
}

struct FakeSpace(AtomicU64);
impl SpaceProbe for FakeSpace {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

// REQ 12.2
#[test]
fn bounded_batches_check_normalized_text_space_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(FakeSpace(AtomicU64::new(u64::MAX)));
    let options = OpenOptions {
        space_probe: Some(probe.clone()),
        ..OpenOptions::default()
    };
    let mut db = Engine::create(
        &dir.path().join("import.db"),
        DataOrigin::Synthetic,
        &options,
    )
    .unwrap();
    let error = db
        .import_companies(&vec![company(); 501], DuplicatePolicy::Error)
        .unwrap_err();
    assert_eq!(error.row, 0);
    assert!(matches!(error.error, EngineError::InvalidInput { .. }));
    assert_eq!(db.count(&Filters::default()).unwrap().n, 0);
    probe.0.store(8 * 1024 * 1024, Ordering::SeqCst);
    assert_eq!(
        db.import_companies(&vec![company(); 500], DuplicatePolicy::Error)
            .unwrap()
            .inserted,
        500
    );
    let large = CompanyInput {
        name: "㍿".repeat(50_000),
        ..company()
    };
    let error = db
        .import_companies(&[large], DuplicatePolicy::Error)
        .unwrap_err();
    assert_eq!(error.row, 0);
    assert!(matches!(error.error, EngineError::InsufficientSpace));
    assert_eq!(db.count(&Filters::default()).unwrap().n, 500);
    probe.0.store(0, Ordering::SeqCst);
    assert_eq!(
        db.import_companies(&[], DuplicatePolicy::Error).unwrap(),
        ImportBatchReport::default()
    );
    let error = db
        .import_companies(&[company()], DuplicatePolicy::Error)
        .unwrap_err();
    assert_eq!(error.row, 0);
    assert!(matches!(error.error, EngineError::InsufficientSpace));
    verify(&db);
}
