use super::support::*;
use meibo_core::{DuplicatePolicy, Engine, OpenOptions, types::*};

// REQ 13.1
#[test]
fn visitor_masks_personal_fields_and_always_omits_erased_companies() {
    let (_dir, mut db) = database();
    let ordinary = db.register_company(&company()).unwrap();
    let excluded = db
        .register_company(&CompanyInput {
            status: Status::Excluded,
            ..company()
        })
        .unwrap();
    let sole_input = CompanyInput {
        is_sole_proprietor: true,
        ..company()
    };
    let sole = db.register_company(&sole_input).unwrap();
    let erased = db.register_company(&sole_input).unwrap();
    db.erase_company(erased).unwrap();
    let mut rows = Vec::new();
    assert_eq!(
        db.visit_companies(&Filters::default(), &vec![], |company| {
            rows.push(company);
            Ok(())
        })
        .unwrap(),
        2
    );
    assert_eq!(
        rows.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![ordinary, sole]
    );
    assert_eq!(rows[0].name, Some(company().name));
    assert!(rows[1].name.is_none() && rows[1].phone.is_none() && rows[1].website.is_none());
    rows.clear();
    assert_eq!(
        db.visit_companies(
            &Filters {
                include_excluded: true,
                ..Filters::default()
            },
            &vec![Field::Name],
            |company| {
                rows.push(company);
                Ok(())
            }
        )
        .unwrap(),
        3
    );
    assert_eq!(
        rows.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![ordinary, excluded, sole]
    );
    assert_eq!(rows[2].name.as_deref(), Some(sole_input.name.as_str()));
    assert!(rows[2].phone.is_none() && rows[2].website.is_none());
    rows.clear();
    db.visit_companies(
        &Filters {
            status: vec![Status::Excluded],
            ..Filters::default()
        },
        &fields(),
        |company| {
            rows.push(company);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![excluded]
    );
    verify(&db);
}

// REQ 13.1
#[test]
fn visitor_keeps_one_snapshot_across_filter_ranges_during_other_writes() {
    let dir = tempfile::tempdir().unwrap();
    let options = OpenOptions {
        background_checkpoint: false,
        ..OpenOptions::default()
    };
    let path = dir.path().join("snapshot.db");
    let mut reader = Engine::create(&path, DataOrigin::Synthetic, &options).unwrap();
    let first = reader.register_company(&company()).unwrap();
    let second_input = CompanyInput {
        name: "更新前合成企業".into(),
        prefecture: 14,
        municipality: Some("14101".into()),
        ..company()
    };
    let second = reader.register_company(&second_input).unwrap();
    let mut writer = Engine::open(&path, &options).unwrap();
    let filters = Filters {
        region: vec![Region::Prefecture(13), Region::Prefecture(14)],
        ..Filters::default()
    };
    let mut rows = Vec::new();
    let count = reader
        .visit_companies(&filters, &fields(), |company| {
            if rows.is_empty() {
                writer.update_company(
                    second,
                    &CompanyInput {
                        name: "更新後合成企業".into(),
                        prefecture: 15,
                        municipality: Some("15101".into()),
                        ..second_input.clone()
                    },
                )?;
                writer.register_company(&second_input)?;
            }
            rows.push(company);
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        rows.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(rows[1].name.as_deref(), Some("更新前合成企業"));
    assert_eq!(rows[1].prefecture, 14);
    assert_eq!(
        reader
            .company_detail(second, &fields())
            .unwrap()
            .company
            .prefecture,
        15
    );
    assert_eq!(reader.count(&Filters::default()).unwrap().n, 3);
    verify(&reader);
}

// REQ 13.1
#[test]
fn visitor_streams_beyond_one_page_and_releases_snapshot_on_errors() {
    let (_dir, mut db) = database();
    db.import_companies(&vec![company(); 250], DuplicatePolicy::Error)
        .unwrap();
    let mut seen = 0;
    assert_eq!(
        db.visit_companies(&Filters::default(), &fields(), |_| {
            seen += 1;
            Ok(())
        })
        .unwrap(),
        250
    );
    assert_eq!(seen, 250);
    let result = db.visit_companies(&Filters::default(), &fields(), |_| {
        Err(EngineError::InvalidInput {
            field: "visitor".into(),
            reason: "injected".into(),
        })
    });
    assert!(matches!(result, Err(EngineError::InvalidInput { field, .. }) if field == "visitor"));
    verify(&db);
    let result = db.visit_companies(
        &Filters {
            region: vec![Region::Prefecture(48)],
            ..Filters::default()
        },
        &fields(),
        |_| panic!("invalid filters must not visit rows"),
    );
    assert!(matches!(result, Err(EngineError::InvalidInput { .. })));
    db.import_companies(&[company()], DuplicatePolicy::Error)
        .unwrap();
    assert_eq!(db.count(&Filters::default()).unwrap().n, 251);
    verify(&db);
}
