use super::support::*;
use meibo_core::types::*;

// REQ 5.1
// REQ 5.2
// REQ 5.4
// REQ 6.5
// REQ 2.2
// REQ 2.3
// REQ 1.2
// REQ 1.6
// api.md の操作表を上から順に追う筋書きなので、分けずに1本で読めるようにする。
#[allow(clippy::too_many_lines)]
#[test]
fn operation_table_and_atomic_failure() {
    let (_dir, mut db) = database();
    let mut c = company();
    c.corporate_number = Some(meibo_core::houjin::generate(0).unwrap());
    let id = db.register_company(&c).unwrap();
    assert!(
        matches!(db.register_company(&c), Err(EngineError::DuplicateCorporateNumber { existing }) if existing == id)
    );
    c.name = "更新済み合成会社".into();
    db.update_company(id, &c).unwrap();
    let person = db
        .add_contact(&ContactInput {
            company: id,
            name: "合成担当者".into(),
            phone: None,
            email: None,
        })
        .unwrap();
    let initial = db.company_detail(id, &fields()).unwrap();
    assert_eq!(initial.company.source_id, c.source_id);
    assert_eq!(initial.company.source_as_of, c.source_as_of);
    assert_eq!(initial.company.name, Some(c.name.clone()));
    let check = |db: &meibo_core::Engine, status, last, next| {
        let d = db.company_detail(id, &fields()).unwrap();
        assert_eq!(
            (
                d.company.status,
                d.company.last_activity_at,
                d.company.next_action_at
            ),
            (status, last, next)
        );
    };
    check(&db, Status::Untouched, 0, None);
    let mut call = activity(id);
    call.kind = ActivityKind::Call;
    call.at = 100;
    call.contact = Some(person);
    call.status_after = Some(Status::InProgress);
    call.next_action = NextAction::Set(500);
    let call_id = db.record_activity(&call).unwrap();
    check(&db, Status::InProgress, 100, Some(500));
    db.record_activity(&activity(id)).unwrap();
    check(&db, Status::InProgress, 100, Some(500));
    let mut change = activity(id);
    change.kind = ActivityKind::StatusChange;
    change.status_after = Some(Status::Negotiating);
    change.next_action = NextAction::Clear;
    let change_id = db.record_activity(&change).unwrap();
    check(&db, Status::Negotiating, 100, None);
    let cancel = |target| ActivityInput {
        kind: ActivityKind::Cancel,
        reverses: Some(target),
        ..activity(id)
    };
    let cancelled = db.record_activity(&cancel(call_id)).unwrap();
    check(&db, Status::Negotiating, 50, None);
    db.record_activity(&cancel(change_id)).unwrap();
    check(&db, Status::Untouched, 50, None);
    let other = db.register_company(&company()).unwrap();
    let other_activity = db.record_activity(&activity(other)).unwrap();
    for target in [cancelled, call_id, other_activity] {
        assert!(matches!(
            db.record_activity(&cancel(target)),
            Err(EngineError::InvalidCancel)
        ));
    }
    assert!(matches!(
        db.record_activity(&ActivityInput {
            company: other,
            contact: Some(person),
            ..activity(other)
        }),
        Err(EngineError::ForeignContact)
    ));
    assert!(matches!(
        db.add_contact(&ContactInput {
            company: CompanyId(i64::MAX),
            name: "合成".into(),
            phone: None,
            email: None
        }),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        db.record_activity(&activity(CompanyId(i64::MAX))),
        Err(EngineError::NotFound)
    ));
    assert!(matches!(
        db.record_activity(&ActivityInput {
            contact: Some(ContactId(i64::MAX)),
            ..activity(id)
        }),
        Err(EngineError::NotFound)
    ));
    let excluded = db
        .register_company(&CompanyInput {
            status: Status::Excluded,
            ..company()
        })
        .unwrap();
    assert!(matches!(
        db.record_activity(&ActivityInput {
            status_after: Some(Status::Won),
            ..activity(excluded)
        }),
        Err(EngineError::ExcludedCompany)
    ));
    db.record_activity(&activity(excluded)).unwrap();
    assert_eq!(
        db.company_detail(excluded, &fields())
            .unwrap()
            .company
            .status,
        Status::Excluded
    );
    db.release_exclusion(excluded, Status::Untouched).unwrap();
    let d = db.company_detail(id, &fields()).unwrap();
    assert_eq!(d.contacts[0].id, person);
    assert!(
        d.activities
            .windows(2)
            .all(|p| (p[0].at, p[0].id.0) > (p[1].at, p[1].id.0))
    );
    let conn = rusqlite::Connection::open(db.path().unwrap()).unwrap();
    conn.execute_batch("CREATE TRIGGER injected_failure BEFORE UPDATE OF status ON companies BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(db.record_activity(&call).is_err());
    assert_eq!(db.company_detail(id, &fields()).unwrap(), d);
    conn.execute_batch("DROP TRIGGER injected_failure;")
        .unwrap();
    verify(&db);
}
