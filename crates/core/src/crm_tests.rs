use super::*;
use crate::{
    OpenOptions,
    types::{CompanyInput, ContactInput, DataOrigin, Fields, Status},
};
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tempfile::TempDir;

struct MutableSpace(AtomicU64);

impl crate::SpaceProbe for MutableSpace {
    fn available(&self, _path: &Path) -> io::Result<u64> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

fn fixture() -> (TempDir, Engine, CompanyId, CompanyId) {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &dir.path().join("crm.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let a = engine
        .register_company(&company("合成取引先A", false))
        .unwrap();
    let b = engine
        .register_company(&company("合成取引先B", false))
        .unwrap();
    (dir, engine, a, b)
}

fn company(name: &str, sole: bool) -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: None,
        name: name.into(),
        prefecture: 13,
        municipality: None,
        industry: None,
        phone: None,
        website: None,
        owner: None,
        status: Status::Untouched,
        is_sole_proprietor: sole,
        source_id: None,
        source_as_of: None,
    }
}

fn task(company: CompanyId, at: i64) -> WorkItemInput {
    WorkItemInput {
        company,
        contact: None,
        kind: WorkKind::Task,
        subject: "合成フォロー".into(),
        due_at: Some(at),
        start_at: None,
        end_at: None,
        completed: false,
    }
}

fn event(company: CompanyId, start: i64) -> WorkItemInput {
    WorkItemInput {
        company,
        contact: None,
        kind: WorkKind::Event,
        subject: "合成打合せ".into(),
        due_at: None,
        start_at: Some(start),
        end_at: Some(start + 3600),
        completed: false,
    }
}

fn opportunity(company: CompanyId) -> OpportunityInput {
    OpportunityInput {
        company,
        contact: None,
        name: "合成案件".into(),
        amount_jpy: 300_000,
        stage: 0,
        close_date: "2026-12-01".into(),
    }
}

fn work_query(company: Option<CompanyId>) -> WorkQuery {
    WorkQuery {
        company,
        kind: None,
        completed: None,
        after: None,
        limit: 200,
    }
}

fn opportunity_query(company: Option<CompanyId>) -> OpportunityQuery {
    OpportunityQuery {
        company,
        after: None,
        limit: 200,
    }
}

#[test]
fn multiple_work_items_complete_reopen_update_and_persist() {
    let (dir, mut engine, a, _) = fixture();
    let late = engine.crm_create_work_item(&task(a, 4000)).unwrap();
    let early = engine.crm_create_work_item(&task(a, 1000)).unwrap();
    let meeting = engine.crm_create_work_item(&event(a, 2000)).unwrap();
    let mut query = work_query(Some(a));
    query.limit = 2;
    let page = engine.crm_work_items(&query).unwrap();
    assert_eq!(
        page.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        [early, meeting]
    );
    assert_eq!(page.total, 3);
    query.after = page.next;
    let last = engine.crm_work_items(&query).unwrap();
    assert_eq!(last.items[0].id, late);
    assert!(last.next.is_none());
    engine.crm_set_completion(early, true).unwrap();
    query = work_query(Some(a));
    query.completed = Some(true);
    assert_eq!(engine.crm_work_items(&query).unwrap().items[0].id, early);
    engine.crm_set_completion(early, false).unwrap();
    assert_eq!(engine.crm_work_items(&query).unwrap().total, 0);
    let mut update = event(a, 2500);
    update.subject = "  合成予定変更  ".into();
    engine.crm_update_work_item(meeting, &update).unwrap();
    drop(engine);
    let engine = Engine::open(&dir.path().join("crm.db"), &OpenOptions::default()).unwrap();
    let items = engine.crm_work_items(&work_query(Some(a))).unwrap().items;
    assert_eq!(items.len(), 3);
    assert_eq!(items[1].input.subject, "合成予定変更");
    assert_eq!(items[1].input.start_at, Some(2500));
    let company = engine
        .company_detail(a, &Fields::default())
        .unwrap()
        .company;
    assert_eq!(company.next_action_at, None);
    assert_eq!(company.status, Status::Untouched);
    assert!(engine.verify().unwrap().counts_match);
}

#[test]
fn opportunities_have_independent_stages_money_dates_and_pages() {
    let (dir, mut engine, a, _) = fixture();
    let first = engine.crm_create_opportunity(&opportunity(a)).unwrap();
    let mut second = opportunity(a);
    second.close_date = "2026-11-01".into();
    second.amount_jpy = MAX_AMOUNT_JPY;
    let second_id = engine.crm_create_opportunity(&second).unwrap();
    let mut query = opportunity_query(Some(a));
    query.limit = 1;
    let page = engine.crm_opportunities(&query).unwrap();
    assert_eq!(page.items[0].id, second_id);
    assert_eq!(page.items[0].input.amount_jpy, MAX_AMOUNT_JPY);
    assert_eq!(page.total, 2);
    query.after = page.next;
    assert_eq!(engine.crm_opportunities(&query).unwrap().items[0].id, first);
    engine.crm_set_stage(first, 4).unwrap();
    let mut update = opportunity(a);
    update.name = "合成変更商談".into();
    update.stage = 5;
    update.amount_jpy = 0;
    engine.crm_update_opportunity(second_id, &update).unwrap();
    drop(engine);
    let engine = Engine::open(&dir.path().join("crm.db"), &OpenOptions::default()).unwrap();
    let items = engine
        .crm_opportunities(&opportunity_query(Some(a)))
        .unwrap()
        .items;
    assert_eq!(
        items.iter().map(|i| i.input.stage).collect::<Vec<_>>(),
        [4, 5]
    );
    assert_eq!(items[1].input.name, "合成変更商談");
    assert_eq!(
        engine
            .company_detail(a, &Fields::default())
            .unwrap()
            .company
            .status,
        Status::Untouched
    );
}

#[test]
fn invalid_inputs_are_atomic_and_foreign_records_cannot_be_moved() {
    let (_dir, mut engine, a, b) = fixture();
    let foreign = engine
        .add_contact(&ContactInput {
            company: b,
            name: "合成担当".into(),
            phone: None,
            email: None,
        })
        .unwrap();
    let mut input = task(a, 100);
    input.contact = Some(foreign);
    assert!(matches!(
        engine.crm_create_work_item(&input),
        Err(EngineError::ForeignContact)
    ));
    input.contact = None;
    let id = engine.crm_create_work_item(&input).unwrap();
    input.company = b;
    assert!(matches!(
        engine.crm_update_work_item(id, &input),
        Err(EngineError::InvalidInput { .. })
    ));
    assert!(matches!(
        engine.crm_update_work_item(id, &event(a, 100)),
        Err(EngineError::InvalidInput { .. })
    ));
    let meeting = engine.crm_create_work_item(&event(a, 100)).unwrap();
    assert!(matches!(
        engine.crm_set_completion(meeting, true),
        Err(EngineError::InvalidInput { .. })
    ));
    for value in ["", "   ", "a\0b"] {
        input = task(a, 100);
        input.subject = value.into();
        assert!(engine.crm_create_work_item(&input).is_err());
    }
    for at in [-1, MAX_TIME + 1] {
        assert!(engine.crm_create_work_item(&task(a, at)).is_err());
    }
    input = event(a, 100);
    input.end_at = Some(100);
    assert!(engine.crm_create_work_item(&input).is_err());
    let oid = engine.crm_create_opportunity(&opportunity(a)).unwrap();
    let mut o = opportunity(b);
    assert!(engine.crm_update_opportunity(oid, &o).is_err());
    o.company = a;
    o.contact = Some(foreign);
    assert!(matches!(
        engine.crm_create_opportunity(&o),
        Err(EngineError::ForeignContact)
    ));
    o.contact = None;
    for bad in [-1, MAX_AMOUNT_JPY + 1] {
        o.amount_jpy = bad;
        assert!(engine.crm_create_opportunity(&o).is_err());
    }
    o.amount_jpy = 0;
    for bad in [
        "2026-02-29",
        "1900-02-29",
        "2026-13-01",
        "0000-01-01",
        "2026-1-01",
        "2026-04-31",
    ] {
        o.close_date = bad.into();
        assert!(engine.crm_create_opportunity(&o).is_err(), "{bad}");
    }
    for good in ["2000-02-29", "2024-02-29", "9999-12-31"] {
        o.close_date = good.into();
        assert!(engine.crm_create_opportunity(&o).is_ok(), "{good}");
    }
    assert!(engine.crm_set_stage(oid, 6).is_err());
    assert_eq!(
        engine.crm_work_items(&work_query(Some(a))).unwrap().total,
        2
    );
    let mut wrong_cursor = work_query(Some(b));
    wrong_cursor.after = Some(id);
    assert!(engine.crm_work_items(&wrong_cursor).is_err());
    let mut wrong_cursor = opportunity_query(Some(b));
    wrong_cursor.after = Some(oid);
    assert!(engine.crm_opportunities(&wrong_cursor).is_err());
}

#[test]
fn contact_and_company_erasure_scrub_related_free_text() {
    let (_dir, mut engine, a, _) = fixture();
    let contact = engine
        .add_contact(&ContactInput {
            company: a,
            name: "合成人物".into(),
            phone: None,
            email: None,
        })
        .unwrap();
    let mut t = task(a, 100);
    t.contact = Some(contact);
    let task_id = engine.crm_create_work_item(&t).unwrap();
    let mut o = opportunity(a);
    o.contact = Some(contact);
    let oid = engine.crm_create_opportunity(&o).unwrap();
    engine.erase_contact(contact, false).unwrap();
    assert_eq!(
        engine.crm_work_items(&work_query(Some(a))).unwrap().total,
        0
    );
    assert_eq!(
        engine
            .crm_opportunities(&opportunity_query(Some(a)))
            .unwrap()
            .total,
        0
    );
    assert!(matches!(
        engine.crm_set_completion(task_id, true),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        engine.crm_set_stage(oid, 1),
        Err(EngineError::NotFound)
    ));
    let stored: (String,String) = engine.db.conn().query_row("SELECT w.subject,o.name FROM crm_work_items w CROSS JOIN crm_opportunities o WHERE w.id=?1 AND o.id=?2",params![task_id,oid],|r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(stored, (String::new(), String::new()));
    let sole = engine
        .register_company(&company("合成個人事業", true))
        .unwrap();
    engine.crm_create_work_item(&task(sole, 100)).unwrap();
    engine.crm_create_opportunity(&opportunity(sole)).unwrap();
    assert!(
        engine
            .crm_work_items(&work_query(Some(sole)))
            .unwrap()
            .items[0]
            .company_name
            .is_none()
    );
    engine.erase_company(sole).unwrap();
    assert_eq!(engine.crm_work_items(&work_query(None)).unwrap().total, 0);
    assert_eq!(
        engine
            .crm_opportunities(&opportunity_query(None))
            .unwrap()
            .total,
        0
    );
    assert!(engine.crm_create_work_item(&task(sole, 100)).is_err());
    assert!(engine.verify().unwrap().counts_match);
}

#[test]
fn legacy_base_database_is_extended_atomically_and_future_extension_rejected() {
    let (dir, engine, a, _) = fixture();
    engine.db.conn().execute_batch("DROP TRIGGER crm_contact_erase;DROP TRIGGER crm_company_erase;DROP TABLE crm_work_items;DROP TABLE crm_opportunities;DELETE FROM meta WHERE key='crm_schema_version';").unwrap();
    drop(engine);
    let path = dir.path().join("crm.db");
    let mut engine = Engine::open(&path, &OpenOptions::default()).unwrap();
    assert!(engine.company_detail(a, &Fields::default()).is_ok());
    engine.crm_create_work_item(&task(a, 100)).unwrap();
    let base: i64 = engine
        .db
        .conn()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(base, 1);
    let backup = dir.path().join("copy.db");
    engine.backup_to(&backup).unwrap();
    let copied = Engine::open(&backup, &OpenOptions::default()).unwrap();
    assert_eq!(copied.crm_work_items(&work_query(None)).unwrap().total, 1);
    drop(copied);
    engine
        .db
        .conn()
        .execute(
            "UPDATE meta SET value='2' WHERE key='crm_schema_version'",
            [],
        )
        .unwrap();
    drop(engine);
    assert!(matches!(
        Engine::open(&path, &OpenOptions::default()),
        Err(EngineError::UnsupportedVersion {
            found: 2,
            supported: 1
        })
    ));
    let conn = Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM crm_work_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let marker: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='crm_schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marker, "2");
}

#[test]
fn crm_domain_supports_real_data_and_preserves_records_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &dir.path().join("real.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )
    .unwrap();
    let mut input = company("合成fixtureを実DB種別で検証", false);
    input.origin = DataOrigin::Real;
    let id = engine.register_company(&input).unwrap();
    let task_id = engine.crm_create_work_item(&task(id, 100)).unwrap();
    engine.crm_create_opportunity(&opportunity(id)).unwrap();
    drop(engine);
    let mut engine = Engine::open(&dir.path().join("real.db"), &OpenOptions::default()).unwrap();
    assert_eq!(
        engine
            .crm_opportunities(&opportunity_query(None))
            .unwrap()
            .total,
        1
    );
    engine.crm_set_completion(task_id, true).unwrap();
    assert_eq!(engine.origin(), DataOrigin::Real);
    assert!(matches!(
        engine.register_company(&company("種別混在", false)),
        Err(EngineError::OriginMismatch)
    ));
}

#[test]
fn initial_extension_checks_space_before_any_schema_change() {
    let (dir, engine, _, _) = fixture();
    engine.db.conn().execute_batch("DROP TRIGGER crm_contact_erase;DROP TRIGGER crm_company_erase;DROP TABLE crm_work_items;DROP TABLE crm_opportunities;DELETE FROM meta WHERE key='crm_schema_version';").unwrap();
    drop(engine);
    let path = dir.path().join("crm.db");
    let probe = Arc::new(MutableSpace(AtomicU64::new(0)));
    let options = OpenOptions {
        space_probe: Some(probe.clone()),
        ..OpenOptions::default()
    };
    assert!(matches!(
        Engine::open(&path, &options),
        Err(EngineError::InsufficientSpace)
    ));
    let conn = Connection::open(&path).unwrap();
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN('crm_work_items','crm_opportunities')",[],|r| r.get(0)).unwrap();
    let marker: i64 = conn
        .query_row(
            "SELECT count(*) FROM meta WHERE key='crm_schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((tables, marker), (0, 0));
    drop(conn);
    probe.0.store(u64::MAX, Ordering::Relaxed);
    let engine = Engine::open(&path, &options).unwrap();
    drop(engine);
    // 拡張済みなら空き不足でも新たなDDL領域は求めずに開ける。
    probe.0.store(0, Ordering::Relaxed);
    assert!(Engine::open(&path, &options).is_ok());
}

#[test]
fn adding_custom_extension_to_an_existing_crm_checks_space_first() {
    let (dir, engine, _, _) = fixture();
    engine.db.conn().execute_batch("DROP TABLE custom_field_values;DROP TABLE custom_field_definitions;DELETE FROM meta WHERE key='custom_fields_schema_version';").unwrap();
    drop(engine);
    let probe = Arc::new(MutableSpace(AtomicU64::new(0)));
    let options = OpenOptions {
        space_probe: Some(probe),
        ..OpenOptions::default()
    };
    let path = dir.path().join("crm.db");
    assert!(matches!(
        Engine::open(&path, &options),
        Err(EngineError::InsufficientSpace)
    ));
    let db = Connection::open(path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name='custom_field_definitions'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn erasing_many_related_crm_records_checks_space_and_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(MutableSpace(AtomicU64::new(u64::MAX)));
    let options = OpenOptions {
        space_probe: Some(probe.clone()),
        ..OpenOptions::default()
    };
    let mut engine =
        Engine::create(&dir.path().join("crm.db"), DataOrigin::Synthetic, &options).unwrap();
    let company = engine
        .register_company(&company("合成個人事業", true))
        .unwrap();
    let contact = engine
        .add_contact(&ContactInput {
            company,
            name: "合成担当".into(),
            phone: None,
            email: None,
        })
        .unwrap();
    for index in 0..40 {
        let mut input = task(company, 100 + index);
        input.contact = Some(contact);
        input.subject = "合".repeat(200);
        engine.crm_create_work_item(&input).unwrap();
        let mut input = opportunity(company);
        input.contact = Some(contact);
        input.name = "成".repeat(200);
        engine.crm_create_opportunity(&input).unwrap();
    }
    // 旧見積（基準1MiB+活動/連絡先数ページ）は通るが、80件のCRM更新には足りない。
    probe.0.store(WRITE_BASE + 64 * 1024, Ordering::Relaxed);
    assert!(matches!(
        engine.erase_contact(contact, false),
        Err(EngineError::InsufficientSpace)
    ));
    assert!(matches!(
        engine.erase_company(company),
        Err(EngineError::InsufficientSpace)
    ));
    let detail = engine
        .company_detail(company, &vec![crate::types::Field::ContactName])
        .unwrap();
    assert_eq!(detail.contacts[0].name.as_deref(), Some("合成担当"));
    assert!(detail.company.erased_at.is_none());
    assert_eq!(
        engine
            .crm_work_items(&work_query(Some(company)))
            .unwrap()
            .total,
        40
    );
    assert_eq!(
        engine
            .crm_opportunities(&opportunity_query(Some(company)))
            .unwrap()
            .total,
        40
    );
    probe.0.store(u64::MAX, Ordering::Relaxed);
    engine.erase_company(company).unwrap();
    assert_eq!(engine.crm_work_items(&work_query(None)).unwrap().total, 0);
    assert_eq!(
        engine
            .crm_opportunities(&opportunity_query(None))
            .unwrap()
            .total,
        0
    );
}
