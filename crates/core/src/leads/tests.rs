use super::*;
use crate::{OpenOptions, types::DataOrigin};

const KEY: &str = "fedcba9876543210fedcba9876543210";

fn setup() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let db = Engine::create(
        &dir.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    (dir, db)
}
fn inquiry(n: i64) -> Inquiry {
    Inquiry {
        submission_key: format!("{n:032x}"),
        company_name: "合成商事".into(),
        contact_name: "テスト担当".into(),
        email: "test@example.invalid".into(),
        message: Some("合成問い合わせ".into()),
        consent: true,
    }
}
fn conversion() -> Conversion {
    Conversion {
        revision: 1,
        owner_id: None,
        company_id: None,
        prefecture: Some(13),
        sole_proprietor: false,
        opportunity: Some(NewOpportunity {
            name: "合成商談".into(),
            amount_jpy: 1234,
            close_date: "2026-12-01".into(),
        }),
    }
}
fn count(db: &Engine, table: &str) -> i64 {
    db.db
        .conn()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

// Requirement 20.6: opt-in intake, safe replay, consent, limits, and rotation.
#[test]
fn public_intake_is_opt_in_bounded_and_staff_only_after_submission() {
    let (_dir, mut db) = setup();
    assert!(matches!(
        db.receive_lead(KEY, &inquiry(1), 900),
        Err(EngineError::NotFound)
    ));
    db.set_lead_form(Some(KEY)).unwrap();
    let mut denied = inquiry(1);
    denied.consent = false;
    assert!(db.receive_lead(KEY, &denied, 900).is_err());
    denied = inquiry(1);
    denied.contact_name = "a".repeat(101);
    assert!(db.receive_lead(KEY, &denied, 900).is_err());
    for id in 1..=30 {
        db.receive_lead(KEY, &inquiry(id), 900).unwrap();
    }
    db.receive_lead(KEY, &inquiry(1), 900).unwrap();
    let mut changed = inquiry(1);
    changed.email = "changed@example.invalid".into();
    assert!(matches!(
        db.receive_lead(KEY, &changed, 900),
        Err(EngineError::RevisionConflict)
    ));
    assert_eq!(
        db.lead(1, true).unwrap().email.as_deref(),
        Some("test@example.invalid")
    );
    assert!(
        matches!(db.receive_lead(KEY,&inquiry(31),900),Err(EngineError::InvalidInput{field,..}) if field=="inbox_capacity")
    );
    db.receive_lead(KEY, &inquiry(31), 1801).unwrap();
    assert_eq!(count(&db, "crm_leads"), 31);
    assert_eq!(count(&db, "companies"), 0);
    assert_eq!(count(&db, "contacts"), 0);
    assert_eq!(count(&db, "crm_opportunities"), 0);
    assert!(
        db.leads(0, None)
            .unwrap()
            .iter()
            .all(|v| v.contact_name.is_none() && v.email.is_none())
    );
    assert!(db.lead(1, false).unwrap().message.is_none());
    assert_eq!(
        db.lead(1, true).unwrap().contact_name.as_deref(),
        Some("テスト担当")
    );
    db.set_lead_form(None).unwrap();
    db.set_lead_form(Some("11111111111111111111111111111111"))
        .unwrap();
    assert!(matches!(
        db.receive_lead(KEY, &inquiry(32), 1900),
        Err(EngineError::NotFound)
    ));
}

// Requirement 20.7: failures roll back, owner conflicts require reread, replay returns the original IDs.
#[test]
fn conversion_is_atomic_idempotent_and_clears_personal_copies() {
    let (_dir, mut db) = setup();
    db.set_lead_form(Some(KEY)).unwrap();
    db.receive_lead(KEY, &inquiry(1), 900).unwrap();
    let mut req = conversion();
    db.assign_lead(1, 1, Some(7)).unwrap();
    assert!(matches!(
        db.convert_lead(1, &req),
        Err(EngineError::RevisionConflict)
    ));
    req.owner_id = Some(7);
    req.revision = 2;
    req.opportunity.as_mut().unwrap().close_date = "2026-02-30".into();
    assert!(db.convert_lead(1, &req).is_err());
    for table in ["companies", "contacts", "activities", "crm_opportunities"] {
        assert_eq!(count(&db, table), 0);
    }
    assert_eq!(db.lead(1, true).unwrap().state, 0);
    req.opportunity.as_mut().unwrap().close_date = "2026-12-01".into();
    let first = db.convert_lead(1, &req).unwrap();
    let again = db.convert_lead(1, &req).unwrap();
    assert_eq!(
        (first.company_id, first.contact_id, first.opportunity_id),
        (again.company_id, again.contact_id, again.opportunity_id)
    );
    assert_eq!(first.state, 1);
    let receipt = db.lead(1, true).unwrap();
    assert!(
        receipt.company_name.is_none()
            && receipt.contact_name.is_none()
            && receipt.email.is_none()
            && receipt.message.is_none()
    );
    for table in ["companies", "contacts", "activities", "crm_opportunities"] {
        assert_eq!(count(&db, table), 1);
    }
    db.receive_lead(KEY, &inquiry(2), 901).unwrap();
    let mut link = conversion();
    assert!(db.convert_lead(2, &link).is_err()); // Same-name candidate needs a deliberate choice.
    link.company_id = first.company_id;
    link.prefecture = None;
    link.opportunity = None;
    let linked = db.convert_lead(2, &link).unwrap();
    assert_eq!(linked.contact_id, first.contact_id);
    assert!(linked.opportunity_id.is_none());
    db.receive_lead(KEY, &inquiry(3), 902).unwrap();
    db.discard_lead(3, 1).unwrap();
    assert!(db.lead(3, true).unwrap().email.is_none());
    db.receive_lead(KEY, &inquiry(3), 903).unwrap();
    assert!(matches!(
        db.convert_lead(3, &link),
        Err(EngineError::RevisionConflict)
    ));
    assert!(db.leads(0, None).unwrap().is_empty());
}

#[test]
fn excluded_and_erased_accounts_cannot_receive_leads() {
    let (_dir, mut db) = setup();
    db.set_lead_form(Some(KEY)).unwrap();
    db.receive_lead(KEY, &inquiry(1), 900).unwrap();
    let mut first = conversion();
    first.sole_proprietor = true;
    let converted = db.convert_lead(1, &first).unwrap();
    let company = crate::types::CompanyId(converted.company_id.unwrap());
    db.receive_lead(KEY, &inquiry(2), 901).unwrap();
    let mut reuse = conversion();
    reuse.company_id = Some(company.0);
    reuse.prefecture = None;
    reuse.opportunity = None;
    db.record_activity(&crate::types::ActivityInput {
        company,
        contact: None,
        kind: crate::types::ActivityKind::StatusChange,
        at: 901,
        note: None,
        status_after: Some(crate::types::Status::Excluded),
        next_action: crate::types::NextAction::Keep,
        reverses: None,
    })
    .unwrap();
    assert!(matches!(
        db.convert_lead(2, &reuse),
        Err(EngineError::ExcludedCompany)
    ));
    db.erase_company(company).unwrap();
    assert!(matches!(
        db.convert_lead(2, &reuse),
        Err(EngineError::NotFound)
    ));
    assert_eq!(db.lead(2, true).unwrap().state, 0);
}

// Two separately opened writers may convert the same lead after reading its first revision.
#[test]
fn concurrent_conversion_returns_one_receipt_and_writes_one_of_each_business_record() {
    let (dir, mut db) = setup();
    db.set_lead_form(Some(KEY)).unwrap();
    db.receive_lead(KEY, &inquiry(1), 123).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut tasks = vec![];
    for _ in 0..2 {
        let path = dir.path().join("synthetic.db");
        let barrier = barrier.clone();
        tasks.push(std::thread::spawn(move || {
            barrier.wait();
            let mut connection = Engine::open(&path, &OpenOptions::default()).unwrap();
            let result = connection.convert_lead(1, &conversion()).unwrap();
            (result.company_id, result.contact_id, result.opportunity_id)
        }));
    }
    let ids = tasks
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids[0], ids[1]);
    for table in ["companies", "contacts", "activities", "crm_opportunities"] {
        assert_eq!(count(&db, table), 1);
    }
}
