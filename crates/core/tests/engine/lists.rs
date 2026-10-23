use super::support::*;
use meibo_core::{synth::Profile, types::*};
use std::time::Instant;

// REQ 6.1
// REQ 6.2
// REQ 6.3
// REQ 6.4
// REQ 6.5
#[test]
fn frozen_lists_bulk_atomicity_and_exclusions() {
    let (_dir, mut db) = database();
    let a = db.register_company(&company()).unwrap();
    let b = db.register_company(&company()).unwrap();
    let excluded = db
        .register_company(&CompanyInput {
            status: Status::Excluded,
            ..company()
        })
        .unwrap();
    let declared = Filters {
        region: vec![Region::Prefecture(13)],
        ..Filters::default()
    };
    let slow = Filters {
        has_phone: Some(true),
        ..declared.clone()
    };
    assert!(db.count(&slow).unwrap().slow);
    let l1 = db.create_list("declared", &declared, 100).unwrap();
    let l2 = db.create_list("slow", &slow, 100).unwrap();
    let mut change = BulkChange {
        owner: OwnerChange::Set(UserId(7)),
        status: Some(Status::Won),
        include_excluded: false,
    };
    assert_eq!(db.bulk_update(l1, &change).unwrap(), 2);
    assert_eq!(db.bulk_update(l2, &change).unwrap(), 2);
    for id in [a, b] {
        let d = db.company_detail(id, &fields()).unwrap();
        assert_eq!(
            (d.company.owner, d.company.status, d.activities.len()),
            (Some(UserId(7)), Status::Won, 1)
        );
    }
    assert_eq!(
        db.company_detail(excluded, &fields())
            .unwrap()
            .company
            .status,
        Status::Excluded
    );
    let inclusive = db
        .create_list(
            "all",
            &Filters {
                include_excluded: true,
                ..Filters::default()
            },
            100,
        )
        .unwrap();
    let before: Vec<_> = [a, b, excluded]
        .iter()
        .map(|id| db.company_detail(*id, &fields()).unwrap())
        .collect();
    change.include_excluded = true;
    change.owner = OwnerChange::Clear;
    assert!(matches!(
        db.bulk_update(inclusive, &change),
        Err(EngineError::ExcludedCompany)
    ));
    for d in &before {
        assert_eq!(db.company_detail(d.company.id, &fields()).unwrap(), *d);
    }
    let conn = rusqlite::Connection::open(db.path().unwrap()).unwrap();
    conn.execute_batch("CREATE TRIGGER bulk_failure BEFORE UPDATE OF status ON companies WHEN OLD.id=2 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    change.include_excluded = false;
    change.status = Some(Status::InProgress);
    assert!(db.bulk_update(l1, &change).is_err());
    for d in &before {
        assert_eq!(db.company_detail(d.company.id, &fields()).unwrap(), *d);
    }
    conn.execute_batch("DROP TRIGGER bulk_failure;").unwrap();
    for list in [l1, l2, inclusive] {
        db.delete_list(list).unwrap();
    }
    for d in &before {
        assert_eq!(db.company_detail(d.company.id, &fields()).unwrap(), *d);
    }
    verify(&db);
}

// REQ 6.1
// REQ 6.2
#[test]
fn hundred_thousand_list_and_bulk_timing() {
    let (_dir, mut db) = database();
    db.load_synthetic(config(100_000, Profile::Dense)).unwrap();
    let start = Instant::now();
    let list = db
        .create_list(
            "100k",
            &Filters {
                include_excluded: true,
                ..Filters::default()
            },
            100_000,
        )
        .unwrap();
    eprintln!("Engine 100k list debug: {:?}", start.elapsed());
    let start = Instant::now();
    assert_eq!(
        db.bulk_update(
            list,
            &BulkChange {
                owner: OwnerChange::Set(UserId(11)),
                status: Some(Status::Excluded),
                include_excluded: true
            }
        )
        .unwrap(),
        100_000
    );
    eprintln!("Engine 100k bulk debug: {:?}", start.elapsed());
    assert_eq!(
        db.count(&Filters {
            status: vec![Status::Excluded],
            ..Filters::default()
        })
        .unwrap()
        .n,
        100_000
    );
    let rows = all(
        &db,
        query(
            Filters {
                include_excluded: true,
                ..Filters::default()
            },
            Sort::ShapeDefault,
        ),
    );
    assert!(
        rows.iter()
            .all(|c| c.owner == Some(UserId(11)) && c.status == Status::Excluded)
    );
    verify(&db);
}
