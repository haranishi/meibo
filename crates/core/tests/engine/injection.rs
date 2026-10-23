use super::support::*;
use meibo_core::{normalize, types::*};

// REQ 10.5
#[test]
fn hostile_values_are_literal_data() {
    let (_dir, mut db) = database();
    let values = vec![
        "'; DROP TABLE companies; --".to_owned(),
        "\" OR \"1\"=\"1".to_owned(),
        "\"".to_owned(),
        "*".to_owned(),
        "NEAR(".to_owned(),
        "OR".to_owned(),
        "x".repeat(100_000),
    ];
    let mut saved = Vec::new();
    for value in &values {
        let c = CompanyInput {
            name: value.clone(),
            phone: Some(value.clone()),
            website: Some(value.clone()),
            source_id: Some(value.clone()),
            ..company()
        };
        let id = db.register_company(&c).unwrap();
        let person = db
            .add_contact(&ContactInput {
                company: id,
                name: value.clone(),
                phone: Some(value.clone()),
                email: Some(value.clone()),
            })
            .unwrap();
        db.record_activity(&ActivityInput {
            contact: Some(person),
            note: Some(value.clone()),
            ..activity(id)
        })
        .unwrap();
        let list = db.create_list(value, &Filters::default(), 100).unwrap();
        db.delete_list(list).unwrap();
        let d = db.company_detail(id, &fields()).unwrap();
        assert_eq!(d.company.name.as_ref(), Some(value));
        assert_eq!(d.company.phone.as_ref(), Some(value));
        assert_eq!(d.company.website.as_ref(), Some(value));
        assert_eq!(d.contacts[0].name.as_ref(), Some(value));
        assert_eq!(d.contacts[0].phone.as_ref(), Some(value));
        assert_eq!(d.contacts[0].email.as_ref(), Some(value));
        assert_eq!(d.company.source_id.as_ref(), Some(value));
        assert_eq!(d.activities[0].note.as_ref(), Some(value));
        saved.push((id, normalize::name_key(value)));
    }
    for value in &values {
        if normalize::search_term(value).is_err() {
            assert!(matches!(
                db.search_name(value, &search()),
                Err(EngineError::InvalidSearchTerm)
            ));
        } else {
            let expected: Vec<_> = saved
                .iter()
                .filter(|(_, name)| name.contains(&normalize::name_key(value)))
                .map(|(id, _)| *id)
                .collect();
            let result = db.search_name(value, &search()).unwrap();
            assert_eq!(
                result.rows.iter().map(|c| c.id).collect::<Vec<_>>(),
                expected,
                "term={:?}",
                &value[..value.len().min(40)]
            );
            assert_eq!(result.n, Some(expected.len() as u64));
        }
    }
    assert_eq!(
        db.count(&Filters::default()).unwrap().n,
        values.len() as u64
    );
    verify(&db);
}

// REQ 10.5
#[test]
fn nul_values_are_rejected_without_saving() {
    let (_dir, mut db) = database();
    let id = db.register_company(&company()).unwrap();
    let before = db.company_detail(id, &fields()).unwrap();
    let conn = rusqlite::Connection::open(db.path().unwrap()).unwrap();
    for field in ["name", "phone", "website", "source_id"] {
        let mut c = company();
        match field {
            "name" => c.name = "synthetic\0tail".into(),
            "phone" => c.phone = Some("synthetic\0tail".into()),
            "website" => c.website = Some("synthetic\0tail".into()),
            "source_id" => c.source_id = Some("synthetic\0tail".into()),
            _ => unreachable!(),
        }
        assert_nul(db.register_company(&c), field);
        assert_nul(db.update_company(id, &c), field);
    }
    for field in ["name", "phone", "email"] {
        let mut c = ContactInput {
            company: id,
            name: "synthetic".into(),
            phone: None,
            email: None,
        };
        match field {
            "name" => c.name = "synthetic\0tail".into(),
            "phone" => c.phone = Some("synthetic\0tail".into()),
            "email" => c.email = Some("synthetic\0tail".into()),
            _ => unreachable!(),
        }
        assert_nul(db.add_contact(&c), field);
    }
    assert_nul(
        db.record_activity(&ActivityInput {
            note: Some("synthetic\0tail".into()),
            status_after: Some(Status::Won),
            next_action: NextAction::Set(100),
            ..activity(id)
        }),
        "note",
    );
    assert_nul(
        db.create_list("synthetic\0tail", &Filters::default(), 100),
        "name",
    );
    assert_eq!(db.company_detail(id, &fields()).unwrap(), before);
    assert_eq!(db.count(&Filters::default()).unwrap().n, 1);
    let lists: i64 = conn
        .query_row("SELECT count(*) FROM lists", [], |r| r.get(0))
        .unwrap();
    let members: i64 = conn
        .query_row("SELECT count(*) FROM list_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!((lists, members), (0, 0));
    let page = db.search_name("synthetic\0tail", &search()).unwrap();
    assert_eq!(page.rows, vec![]);
    assert_eq!(page.n, Some(0));
    verify(&db);
}

fn assert_nul<T: std::fmt::Debug>(result: Result<T, EngineError>, expected: &str) {
    match result {
        Err(EngineError::InvalidInput { field, reason }) => {
            assert_eq!(
                (field.as_str(), reason.as_str()),
                (expected, "must not contain NUL")
            );
        }
        result => panic!("{result:?}"),
    }
}
