use super::*;
use crate::{
    schema::OpenOptions,
    types::{DataOrigin, Industry, UserId},
};
use proptest::prelude::*;
use tempfile::TempDir;

fn database() -> (TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::create(
        &dir.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    (dir, db)
}

fn company() -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: None,
        name: " 合成あおぞら商事Ａ ".into(),
        prefecture: 1,
        municipality: Some("01101".into()),
        industry: Some(Industry::A),
        phone: None,
        website: None,
        owner: Some(UserId(7)),
        status: Status::Untouched,
        is_sole_proprietor: false,
        source_id: Some("synthetic".into()),
        source_as_of: Some("2024-02-29".into()),
    }
}

fn activity(
    company: CompanyId,
    kind: ActivityKind,
    at: i64,
    after: Option<Status>,
    next: NextAction,
) -> ActivityInput {
    ActivityInput {
        company,
        contact: None,
        kind,
        at,
        note: Some("合成メモ".into()),
        status_after: after,
        next_action: next,
        reverses: None,
    }
}

fn cancel(company: CompanyId, target: ActivityId) -> ActivityInput {
    let mut a = activity(company, ActivityKind::Cancel, 900, None, NextAction::Keep);
    a.reverses = Some(target);
    a
}

fn saved(db: &Db, id: CompanyId) -> Folded {
    company_state(db.conn(), id).unwrap().folded
}

fn assert_saved(
    db: &Db,
    id: CompanyId,
    status: Status,
    last_activity_at: i64,
    next_action_at: Option<i64>,
) {
    assert_eq!(
        saved(db, id),
        Folded {
            status,
            last_activity_at,
            next_action_at
        }
    );
    assert!(verify_folds(db.conn()).unwrap());
    assert!(counts::matches_recount(db.conn()));
}

fn activity_count(db: &Db) -> i64 {
    db.conn()
        .query_row("SELECT count(*) FROM activities", [], |r| r.get(0))
        .unwrap()
}

fn count_rows(db: &Db) -> Vec<(u8, bool, i64)> {
    db.conn()
        .prepare("SELECT status,has_phone,n FROM counts ORDER BY status,has_phone")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

// REQ 5.1, 5.4, 6.5
#[test]
fn operation_table_nine_rows() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    assert_saved(&db, id, Status::Untouched, 0, None); // 1
    let call = record_activity(
        &mut db,
        &activity(
            id,
            ActivityKind::Call,
            100,
            Some(Status::InProgress),
            NextAction::Set(500),
        ),
    )
    .unwrap();
    assert_saved(&db, id, Status::InProgress, 100, Some(500)); // 2
    record_activity(
        &mut db,
        &activity(id, ActivityKind::Memo, 50, None, NextAction::Keep),
    )
    .unwrap();
    assert_saved(&db, id, Status::InProgress, 100, Some(500)); // 3
    let change = record_activity(
        &mut db,
        &activity(
            id,
            ActivityKind::StatusChange,
            1000,
            Some(Status::Negotiating),
            NextAction::Clear,
        ),
    )
    .unwrap();
    assert_saved(&db, id, Status::Negotiating, 100, None); // 4
    let cancellation = record_activity(&mut db, &cancel(id, call)).unwrap();
    assert_saved(&db, id, Status::Negotiating, 50, None); // 5
    record_activity(&mut db, &cancel(id, change)).unwrap();
    assert_saved(&db, id, Status::Untouched, 50, None); // 6
    let other = register_company(&mut db, &company()).unwrap();
    let other_activity = record_activity(
        &mut db,
        &activity(other, ActivityKind::Memo, 1, None, NextAction::Keep),
    )
    .unwrap();
    for target in [cancellation, call, other_activity] {
        // 7
        assert!(matches!(
            record_activity(&mut db, &cancel(id, target)),
            Err(EngineError::InvalidCancel)
        ));
    }
    let mut excluded = company();
    excluded.status = Status::Excluded;
    let excluded = register_company(&mut db, &excluded).unwrap();
    let before = activity_count(&db);
    assert!(matches!(
        record_activity(
            &mut db,
            &activity(
                excluded,
                ActivityKind::StatusChange,
                20,
                Some(Status::Won),
                NextAction::Keep
            )
        ),
        Err(EngineError::ExcludedCompany)
    )); // 8
    assert_eq!(activity_count(&db), before);
    record_activity(
        &mut db,
        &activity(excluded, ActivityKind::Memo, 30, None, NextAction::Keep),
    )
    .unwrap();
    assert_saved(&db, excluded, Status::Excluded, 30, None); // 9
}

fn field_error(result: Result<NormalizedCompany, EngineError>, expected: &str) {
    assert!(matches!(result,Err(EngineError::InvalidInput {field,..}) if field==expected));
}

// REQ 1.1, 1.4, 1.5, 1.7, 1.8
#[test]
fn validates_names_numbers_regions_and_calendar() {
    let mut c = company();
    c.name = " \t\u{3000}".into();
    assert!(matches!(
        validate_company_input(&c),
        Err(EngineError::EmptyName)
    ));
    c = company();
    for number in [
        "",
        " 6600000000000",
        "6600000000000 ",
        "0600000000000",
        "６６０００００００００００",
    ] {
        c.corporate_number = Some(number.into());
        assert!(matches!(
            validate_company_input(&c),
            Err(EngineError::InvalidCorporateNumber { .. })
        ));
    }
    c = company();
    for prefecture in [0, 48, 255] {
        c.prefecture = prefecture;
        field_error(validate_company_input(&c), "prefecture");
    }
    c = company();
    for municipality in ["0110", "011010", "01A01", "０１１０１", "02101", " 01101"] {
        c.municipality = Some(municipality.into());
        field_error(validate_company_input(&c), "municipality");
    }
    c = company();
    for date in [
        "2023-02-29",
        "1900-02-29",
        "2024-04-31",
        "2024-00-01",
        "2024-13-01",
        "2024-01-00",
        "0000-01-01",
        "2024-1-01",
        " 2024-01-01",
        "２０２４-01-01",
    ] {
        c.source_as_of = Some(date.into());
        field_error(validate_company_input(&c), "source_as_of");
    }
    for date in ["2000-02-29", "2024-02-29", "0001-01-01", "9999-12-31"] {
        c.source_as_of = Some(date.into());
        assert!(validate_company_input(&c).is_ok());
    }
    c = company();
    c.phone = Some("　０１２０-０００-００１　".into());
    c.website = Some(" https://synthetic.invalid/ ".into());
    let n = validate_company_input(&c).unwrap();
    assert_eq!(n.company.name, c.name);
    assert_eq!(n.name_key, normalize::name_key(&c.name));
    assert_eq!(n.company.phone.as_deref(), Some("0120-000-001"));
    assert_eq!(
        n.company.website.as_deref(),
        Some("https://synthetic.invalid/")
    );
}

// REQ 1.1, 1.2, 1.3, 1.5, 1.6, 1.7, 1.8, 8.3
// REQ 1.2
// REQ 1.6
#[test]
fn registration_update_duplicates_and_preserved_fields() {
    let (_dir, mut db) = database();
    let mut c = company();
    c.corporate_number = Some(houjin::generate(0).unwrap());
    c.phone = Some(" ０１２０-０００-００１ ".into());
    let id = register_company(&mut db, &c).unwrap();
    assert!(
        matches!(register_company(&mut db,&c),Err(EngineError::DuplicateCorporateNumber {existing}) if existing==id)
    );
    update_company(&mut db, id, &c).unwrap();
    let other = register_company(&mut db, &company()).unwrap();
    assert!(
        matches!(update_company(&mut db,other,&c),Err(EngineError::DuplicateCorporateNumber {existing}) if existing==id)
    );
    c.origin = DataOrigin::Real;
    assert!(matches!(
        register_company(&mut db, &c),
        Err(EngineError::OriginMismatch)
    ));
    assert!(matches!(
        update_company(&mut db, id, &c),
        Err(EngineError::OriginMismatch)
    ));
    c.origin = DataOrigin::Synthetic;
    let row: (String, String, String, String, String) = db
        .conn()
        .query_row(
            "SELECT corporate_number,phone,name,source_id,source_as_of FROM companies WHERE id=?1",
            [id.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            c.corporate_number.clone().unwrap(),
            "0120-000-001".into(),
            c.name.clone(),
            "synthetic".into(),
            "2024-02-29".into()
        )
    );
    // 検査用数字は1〜9なので、先頭0の13桁番号は拒否する。
    assert!(matches!(
        houjin::validate("0600000000000"),
        Err(EngineError::InvalidCorporateNumber { .. })
    ));
    assert!(counts::matches_recount(db.conn()));
}

type UpdatedFields = (
    Option<String>,
    u8,
    String,
    u8,
    Option<i64>,
    bool,
    String,
    Option<String>,
);

// REQ 1.3, 1.6, 1.7, 5.1
#[test]
fn company_update_preserves_activity_state_and_rejects_erased() {
    let (_dir, mut db) = database();
    let mut c = company();
    c.corporate_number = Some(houjin::generate(0).unwrap());
    let id = register_company(&mut db, &c).unwrap();
    record_activity(
        &mut db,
        &activity(
            id,
            ActivityKind::Call,
            101,
            Some(Status::Won),
            NextAction::Set(606),
        ),
    )
    .unwrap();
    assert!(matches!(
        update_company(&mut db, id, &c),
        Err(EngineError::StatusViaActivity)
    ));
    let before: (i64, u8) = db
        .conn()
        .query_row(
            "SELECT created_at,base_status FROM companies WHERE id=?1",
            [id.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    c.status = Status::Won;
    c.corporate_number = None;
    c.prefecture = 47;
    c.municipality = Some("47201".into());
    c.industry = Some(Industry::T);
    c.owner = None;
    c.is_sole_proprietor = true;
    c.source_id = Some("revised synthetic".into());
    c.source_as_of = None;
    update_company(&mut db, id, &c).unwrap();
    let after: (i64, u8) = db
        .conn()
        .query_row(
            "SELECT created_at,base_status FROM companies WHERE id=?1",
            [id.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_saved(&db, id, Status::Won, 101, Some(606));
    let updated: UpdatedFields = db.conn().query_row("SELECT corporate_number,prefecture_code,municipality_code,industry_code,owner_id,is_sole_proprietor,source_id,source_as_of FROM companies WHERE id=?1",[id.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).unwrap();
    assert_eq!(
        updated,
        (
            None,
            47,
            "47201".into(),
            20,
            None,
            true,
            "revised synthetic".into(),
            None
        )
    );
    assert!(matches!(
        update_company(&mut db, CompanyId(999), &c),
        Err(EngineError::NotFound)
    ));
    db.conn()
        .execute("UPDATE companies SET erased_at=1 WHERE id=?1", [id.0])
        .unwrap();
    assert!(
        matches!(update_company(&mut db,id,&c),Err(EngineError::InvalidInput {field,reason}) if field=="id" && reason=="erased")
    );
    assert!(
        matches!(record_activity(&mut db,&activity(id,ActivityKind::Memo,1,None,NextAction::Keep)),Err(EngineError::InvalidInput {field,reason}) if field=="id" && reason=="erased")
    );
}

fn search_ids(db: &Db, sql: &str, term: &str) -> Vec<i64> {
    db.conn()
        .prepare(sql)
        .unwrap()
        .query_map([term], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn assert_fts(db: &Db) {
    for term in ["合成ア", "ソラ商", "しんせ", "しんせつ", "new", "合成改"] {
        let key = normalize::name_key(term);
        assert_eq!(
            search_ids(
                db,
                "SELECT rowid FROM name_fts WHERE name_fts MATCH ?1 ORDER BY rowid",
                &format!("\"{key}\"")
            ),
            search_ids(
                db,
                "SELECT id FROM companies WHERE instr(name_key,?1)>0 ORDER BY id",
                &key
            )
        );
    }
}

// REQ 1.4, 1.5, 1.7, 4.1, 5.2
#[test]
fn rename_phone_counts_and_fts_are_atomic() {
    let (_dir, mut db) = database();
    let mut c = company();
    c.phone = Some("　　".into());
    c.website = Some(" \t".into());
    let id = register_company(&mut db, &c).unwrap();
    let empty: (Option<String>, Option<String>) = db
        .conn()
        .query_row(
            "SELECT phone,website FROM companies WHERE id=?1",
            [id.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(empty, (None, None));
    assert_fts(&db);
    for (name, phone) in [
        ("合成しんせつ", Some("0120-0")),
        ("合成NEW", None),
        ("合成改名", Some("03-0000-0000")),
    ] {
        c.name = name.into();
        c.phone = phone.map(str::to_owned);
        update_company(&mut db, id, &c).unwrap();
        assert_fts(&db);
        assert!(counts::matches_recount(db.conn()));
        let has: bool = db
            .conn()
            .query_row("SELECT has_phone FROM companies WHERE id=?1", [id.0], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(has, phone.is_some());
        let n: i64 = db
            .conn()
            .query_row(
                "SELECT n FROM counts WHERE status=0 AND has_phone=?1",
                [has],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let original = count_rows(&db);
        counts::recount(db.conn()).unwrap();
        assert_eq!(original, count_rows(&db));
    }
    let before = count_rows(&db);
    db.conn().execute_batch("CREATE TEMP TRIGGER fail_update BEFORE UPDATE ON companies BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    c.name = "合成しんせつ".into();
    c.phone = None;
    assert!(matches!(
        update_company(&mut db, id, &c),
        Err(EngineError::Storage(_))
    ));
    assert_fts(&db);
    assert_eq!(before, count_rows(&db));
    db.conn().execute_batch("DROP TRIGGER fail_update").unwrap();
    c.name = " ".into();
    assert!(matches!(
        update_company(&mut db, id, &c),
        Err(EngineError::EmptyName)
    ));
    assert_fts(&db);
}

// REQ 2.1, 2.3, 1.8
#[test]
fn contacts_validate_names_and_company_relationships() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let other = register_company(&mut db, &company()).unwrap();
    let mut c = ContactInput {
        company: id,
        name: "合成担当者".into(),
        phone: Some(" ０９０-００００-００００ ".into()),
        email: Some(" \t".into()),
    };
    let contact = add_contact(&mut db, &c).unwrap();
    let values: (String, Option<String>) = db
        .conn()
        .query_row(
            "SELECT phone,email FROM contacts WHERE id=?1",
            [contact.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(values, ("090-0000-0000".into(), None));
    let mut a = activity(other, ActivityKind::Call, 10, None, NextAction::Keep);
    a.contact = Some(contact);
    assert!(matches!(
        record_activity(&mut db, &a),
        Err(EngineError::ForeignContact)
    ));
    a.contact = Some(ContactId(999));
    assert!(matches!(
        record_activity(&mut db, &a),
        Err(EngineError::NotFound)
    ));
    a.company = id;
    a.contact = Some(contact);
    record_activity(&mut db, &a).unwrap();
    c.name = "　".into();
    assert!(matches!(
        add_contact(&mut db, &c),
        Err(EngineError::EmptyName)
    ));
    c.name = "合成".into();
    c.company = CompanyId(999);
    assert!(matches!(
        add_contact(&mut db, &c),
        Err(EngineError::NotFound)
    ));
    db.conn()
        .execute("UPDATE companies SET erased_at=1 WHERE id=?1", [id.0])
        .unwrap();
    c.company = id;
    assert!(matches!(
        add_contact(&mut db, &c),
        Err(EngineError::NotFound)
    ));
    assert_eq!(activity_count(&db), 1);
}

// REQ 5.1, 5.4, 6.5
#[test]
fn cancellation_and_exclusion_validation() {
    let (_dir, mut db) = database();
    let mut c = company();
    c.status = Status::Negotiating;
    let id = register_company(&mut db, &c).unwrap();
    let a = record_activity(
        &mut db,
        &activity(
            id,
            ActivityKind::Email,
            100,
            Some(Status::Won),
            NextAction::Set(500),
        ),
    )
    .unwrap();
    record_activity(&mut db, &cancel(id, a)).unwrap();
    assert_saved(&db, id, Status::Negotiating, 0, None);
    assert!(matches!(
        record_activity(&mut db, &cancel(id, ActivityId(999))),
        Err(EngineError::InvalidCancel)
    ));
    let excluding = record_activity(
        &mut db,
        &activity(
            id,
            ActivityKind::StatusChange,
            101,
            Some(Status::Excluded),
            NextAction::Keep,
        ),
    )
    .unwrap();
    let before = activity_count(&db);
    assert!(matches!(
        record_activity(&mut db, &cancel(id, excluding)),
        Err(EngineError::ExcludedCompany)
    ));
    assert_eq!(activity_count(&db), before);
    assert_saved(&db, id, Status::Excluded, 0, None);
    record_activity(
        &mut db,
        &activity(
            id,
            ActivityKind::Visit,
            20,
            Some(Status::Excluded),
            NextAction::Set(800),
        ),
    )
    .unwrap();
    assert!(matches!(
        release_exclusion(&mut db, id, Status::Excluded),
        Err(EngineError::InvalidInput { .. })
    ));
    let release = release_exclusion(&mut db, id, Status::InProgress).unwrap();
    assert_saved(&db, id, Status::InProgress, 20, Some(800));
    let kind: u8 = db
        .conn()
        .query_row(
            "SELECT kind FROM activities WHERE id=?1",
            [release.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind, 5);
    assert!(
        matches!(release_exclusion(&mut db,id,Status::Won),Err(EngineError::InvalidInput {field,reason}) if field=="status" && reason=="not excluded")
    );
    assert!(verify_folds(db.conn()).unwrap());
}

// REQ 2.3, 5.4
#[test]
fn invalid_activity_shapes_and_unique_cancellation() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let a = record_activity(
        &mut db,
        &activity(id, ActivityKind::Memo, 1, None, NextAction::Keep),
    )
    .unwrap();
    record_activity(&mut db, &cancel(id, a)).unwrap();
    let mut malformed = cancel(id, a);
    malformed.reverses = None;
    assert!(matches!(
        record_activity(&mut db, &malformed),
        Err(EngineError::InvalidInput { .. })
    ));
    malformed = cancel(id, a);
    malformed.status_after = Some(Status::Won);
    assert!(matches!(
        record_activity(&mut db, &malformed),
        Err(EngineError::InvalidInput { .. })
    ));
    malformed = cancel(id, a);
    malformed.next_action = NextAction::Clear;
    assert!(matches!(
        record_activity(&mut db, &malformed),
        Err(EngineError::InvalidInput { .. })
    ));
    malformed = activity(id, ActivityKind::Memo, 1, None, NextAction::Keep);
    malformed.reverses = Some(a);
    assert!(matches!(
        record_activity(&mut db, &malformed),
        Err(EngineError::InvalidInput { .. })
    ));
    malformed = activity(id, ActivityKind::StatusChange, 1, None, NextAction::Keep);
    assert!(matches!(
        record_activity(&mut db, &malformed),
        Err(EngineError::InvalidInput { .. })
    ));
    malformed.company = CompanyId(999);
    assert!(matches!(
        record_activity(&mut db, &malformed),
        Err(EngineError::NotFound)
    ));
    // UNIQUE は公開口を通さない二重取り消しも拒否する。
    let before = activity_count(&db);
    assert!(
        db.write(WRITE_BASE, |tx| {
            tx.execute(
                "INSERT INTO activities(company_id,kind,at,reverses_id) VALUES(?1,9,0,?2)",
                (id.0, a.0),
            )
            .map_err(storage)?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(activity_count(&db), before);
}

// REQ 5.2
#[test]
fn activity_failure_rolls_back_everything() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let before = saved(&db, id);
    let counts = count_rows(&db);
    let n = activity_count(&db);
    db.conn().execute_batch("CREATE TEMP TRIGGER fail_activity BEFORE UPDATE ON companies BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    assert!(matches!(
        record_activity(
            &mut db,
            &activity(
                id,
                ActivityKind::Call,
                100,
                Some(Status::Won),
                NextAction::Set(500)
            )
        ),
        Err(EngineError::Storage(_))
    ));
    assert_eq!(activity_count(&db), n);
    assert_eq!(saved(&db, id), before);
    assert_eq!(count_rows(&db), counts);
    assert!(counts::matches_recount(db.conn()));
    assert!(verify_folds(db.conn()).unwrap());
}

// REQ 5.4
#[test]
fn verify_detects_tampering_for_empty_active_and_cancelled_companies() {
    let (_dir, mut db) = database();
    let empty = register_company(&mut db, &company()).unwrap();
    let active = register_company(&mut db, &company()).unwrap();
    let cancelled = register_company(&mut db, &company()).unwrap();
    record_activity(
        &mut db,
        &activity(active, ActivityKind::Visit, 1, None, NextAction::Set(5)),
    )
    .unwrap();
    let a = record_activity(
        &mut db,
        &activity(cancelled, ActivityKind::Memo, 1, None, NextAction::Keep),
    )
    .unwrap();
    record_activity(&mut db, &cancel(cancelled, a)).unwrap();
    assert!(verify_folds(db.conn()).unwrap());
    for id in [empty, active, cancelled] {
        db.conn()
            .execute("UPDATE companies SET status=3 WHERE id=?1", [id.0])
            .unwrap();
        assert!(!verify_folds(db.conn()).unwrap());
        db.conn()
            .execute("UPDATE companies SET status=0 WHERE id=?1", [id.0])
            .unwrap();
        assert!(verify_folds(db.conn()).unwrap());
    }
    db.conn()
        .execute(
            "UPDATE companies SET next_action_at=99 WHERE id=?1",
            [active.0],
        )
        .unwrap();
    assert!(!verify_folds(db.conn()).unwrap());
}

// REQ 5.1, 5.4
#[test]
fn pure_fold_all_kinds_and_record_order() {
    let entries = [
        FoldInput {
            kind: ActivityKind::Call,
            at: 10,
            status_after: Some(Status::InProgress),
            next_action: NextAction::Set(20),
        },
        FoldInput {
            kind: ActivityKind::Visit,
            at: 30,
            status_after: None,
            next_action: NextAction::Keep,
        },
        FoldInput {
            kind: ActivityKind::Email,
            at: 20,
            status_after: Some(Status::Lost),
            next_action: NextAction::Clear,
        },
        FoldInput {
            kind: ActivityKind::Memo,
            at: 40,
            status_after: None,
            next_action: NextAction::Set(-1),
        },
        FoldInput {
            kind: ActivityKind::StatusChange,
            at: 999,
            status_after: Some(Status::Won),
            next_action: NextAction::Keep,
        },
    ];
    assert_eq!(
        fold(Status::Untouched, entries.into_iter()),
        Folded {
            status: Status::Won,
            last_activity_at: 40,
            next_action_at: Some(-1)
        }
    );
    assert_eq!(
        fold(Status::Lost, std::iter::empty()),
        Folded {
            status: Status::Lost,
            last_activity_at: 0,
            next_action_at: None
        }
    );
}

// 参照側は保存値や本体の fold/apply を使わず、毎回列全体を読み直す。
fn reference(base: Status, log: &[(ActivityId, ActivityInput)]) -> Folded {
    let mut state = base;
    let mut last = 0;
    let mut next = None;
    for (id, a) in log {
        if a.kind == ActivityKind::Cancel || log.iter().any(|(_, r)| r.reverses == Some(*id)) {
            continue;
        }
        if let Some(s) = a.status_after {
            state = s;
        }
        if let NextAction::Set(at) = a.next_action {
            next = Some(at);
        }
        if a.next_action == NextAction::Clear {
            next = None;
        }
        if [
            ActivityKind::Call,
            ActivityKind::Visit,
            ActivityKind::Email,
            ActivityKind::Memo,
        ]
        .contains(&a.kind)
            && a.at > last
        {
            last = a.at;
        }
    }
    Folded {
        status: state,
        last_activity_at: last,
        next_action_at: next,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {cases:64,failure_persistence:None,..ProptestConfig::default()})]
    // REQ 5.1, 5.2, 5.4, 6.5
    #[test]
    fn random_operations_match_reference(ops in prop::collection::vec((0u8..10,0u8..7,0i64..1000,0u8..3,any::<u16>()),1..100), base in 0u8..6) {
        let (_dir, mut db) = database(); let mut c = company(); c.status = Status::from_code(base).unwrap();
        let id = register_company(&mut db,&c).unwrap();
        let other = register_company(&mut db,&company()).unwrap();
        let foreign = record_activity(&mut db,&activity(other,ActivityKind::Memo,1,None,NextAction::Keep)).unwrap();
        let mut log: Vec<(ActivityId,ActivityInput)> = Vec::new();
        for (op,after,at,next,choice) in ops {
            let old = reference(c.status,&log);
            if op == 9 {
                let result = release_exclusion(&mut db,id,Status::Won);
                if old.status == Status::Excluded {
                    let aid = result.unwrap();
                    log.push((aid,activity(id,ActivityKind::StatusChange,0,Some(Status::Won),NextAction::Keep)));
                } else { prop_assert!(matches!(result,Err(EngineError::InvalidInput {..})), "expected invalid release"); }
            } else {
                let kind = match op {0=>ActivityKind::Memo,1=>ActivityKind::Call,2=>ActivityKind::Visit,3=>ActivityKind::Email,4|5=>ActivityKind::StatusChange,_=>ActivityKind::Cancel};
                let after = Status::from_code(after);
                let after = if kind==ActivityKind::StatusChange {Some(after.unwrap_or(Status::Untouched))} else {after};
                let next = match next {0=>NextAction::Keep,1=>NextAction::Clear,_=>NextAction::Set(at*2)};
                let mut a = activity(id,kind,at,after,next);
                if kind == ActivityKind::Cancel {
                    let target = if op==7 {foreign} else if op==8 || log.is_empty() {ActivityId(i64::MAX)} else {log[usize::from(choice)%log.len()].0};
                    a = cancel(id,target);
                }
                let valid_target = a.reverses.is_none_or(|target| log.iter().any(|(aid,entry)|*aid==target && entry.kind!=ActivityKind::Cancel) && !log.iter().any(|(_,entry)|entry.reverses==Some(target)));
                let mut candidate = log.clone(); candidate.push((ActivityId(-1),a.clone()));
                let expected = reference(c.status,&candidate);
                let blocked = old.status==Status::Excluded && (a.status_after.is_some_and(|s|s!=Status::Excluded) || expected.status!=Status::Excluded);
                let before_count = activity_count(&db);
                let result = record_activity(&mut db,&a);
                if !valid_target { prop_assert!(matches!(result,Err(EngineError::InvalidCancel))); }
                else if blocked {prop_assert!(matches!(result,Err(EngineError::ExcludedCompany)));}
                else {let aid=result.unwrap(); log.push((aid,a));}
                prop_assert_eq!(activity_count(&db),before_count + i64::from(valid_target && !blocked));
            }
            prop_assert_eq!(saved(&db,id),reference(c.status,&log));
            prop_assert!(counts::matches_recount(db.conn()));
        }
        prop_assert!(verify_folds(db.conn()).unwrap());
        prop_assert!(counts::matches_recount(db.conn()));
    }
}

// REQ 1.1, 1.3, 1.4, 1.7, 8.3
#[test]
fn invalid_company_inputs_leave_storage_unchanged() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let mut inputs = Vec::new();
    let mut c = company();
    c.name = "　 ".into();
    inputs.push(c);
    let mut c = company();
    c.corporate_number = Some(" 6600000000000".into());
    inputs.push(c);
    let mut c = company();
    c.prefecture = 0;
    inputs.push(c);
    let mut c = company();
    c.municipality = Some("02101".into());
    inputs.push(c);
    let mut c = company();
    c.source_as_of = Some("2023-02-29".into());
    inputs.push(c);
    for c in inputs {
        assert!(register_company(&mut db, &c).is_err());
        assert!(update_company(&mut db, id, &c).is_err());
        assert!(counts::matches_recount(db.conn()));
        assert_eq!(counts::total(db.conn()).unwrap(), 1);
        assert_fts(&db);
    }
    let mut c = company();
    c.corporate_number = Some(houjin::generate(1).unwrap());
    update_company(&mut db, id, &c).unwrap();
    let number: String = db
        .conn()
        .query_row(
            "SELECT corporate_number FROM companies WHERE id=?1",
            [id.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(Some(number), c.corporate_number);
}

fn assert_nul_error<T: std::fmt::Debug>(result: Result<T, EngineError>, expected: &str) {
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

// REQ 1.5, 10.5
#[test]
fn nul_company_fields_reject_registration_update_and_bulk_atomically() {
    use crate::{
        bulk::{self, BulkMode, BulkOptions, BulkRecord},
        lists::tests::snapshot,
    };
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let before = snapshot(db.conn());
    for value in ["\0", "合成\0tail", " \0 "] {
        for field in ["name", "phone", "website", "source_id"] {
            let mut c = company();
            match field {
                "name" => c.name = value.into(),
                "phone" => c.phone = Some(value.into()),
                "website" => c.website = Some(value.into()),
                "source_id" => c.source_id = Some(value.into()),
                _ => unreachable!(),
            }
            assert_nul_error(register_company(&mut db, &c), field);
            assert_eq!(snapshot(db.conn()), before);
            assert_nul_error(update_company(&mut db, id, &c), field);
            assert_eq!(snapshot(db.conn()), before);
            for mode in [BulkMode::Incremental, BulkMode::Bulk] {
                let records = [company(), c.clone()]
                    .into_iter()
                    .map(|company| BulkRecord {
                        company,
                        activities: vec![],
                    });
                assert_nul_error(
                    bulk::load(
                        &mut db,
                        records,
                        Some(2),
                        &BulkOptions {
                            mode,
                            ..BulkOptions::default()
                        },
                    ),
                    field,
                );
                assert_eq!(snapshot(db.conn()), before);
                assert_fts(&db);
            }
        }
    }
}

// REQ 10.5
#[test]
fn nul_contact_fields_leave_storage_unchanged() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let before = crate::lists::tests::snapshot(db.conn());
    for value in ["\0", "合成\0tail", " \0 "] {
        for field in ["name", "phone", "email"] {
            let mut c = ContactInput {
                company: id,
                name: "合成担当者".into(),
                phone: None,
                email: None,
            };
            match field {
                "name" => c.name = value.into(),
                "phone" => c.phone = Some(value.into()),
                "email" => c.email = Some(value.into()),
                _ => unreachable!(),
            }
            assert_nul_error(add_contact(&mut db, &c), field);
            assert_eq!(crate::lists::tests::snapshot(db.conn()), before);
        }
    }
}

// REQ 5.2, 10.5
#[test]
fn nul_activity_note_and_list_name_leave_storage_unchanged() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    let before = crate::lists::tests::snapshot(db.conn());
    for value in ["\0", "合成\0tail", " \0 "] {
        let mut a = activity(
            id,
            ActivityKind::Call,
            100,
            Some(Status::Won),
            NextAction::Set(500),
        );
        a.note = Some(value.into());
        assert_nul_error(record_activity(&mut db, &a), "note");
        assert_eq!(crate::lists::tests::snapshot(db.conn()), before);
        assert_nul_error(
            crate::lists::create_list_from_select(
                &mut db,
                value,
                "SELECT id FROM companies",
                &[],
                100,
            ),
            "name",
        );
        assert_eq!(crate::lists::tests::snapshot(db.conn()), before);
        assert_fts(&db);
    }
}

// REQ 5.1, 5.4
#[test]
fn negative_activity_times_leave_data_unchanged() {
    let (_dir, mut db) = database();
    let id = register_company(&mut db, &company()).unwrap();
    for (at, next, field) in [
        (-1, NextAction::Keep, "at"),
        (0, NextAction::Set(-1), "next_action"),
    ] {
        let before = crate::lists::tests::snapshot(db.conn());
        let a = activity(id, ActivityKind::Memo, at, None, next);
        assert!(
            matches!(record_activity(&mut db, &a), Err(EngineError::InvalidInput { field: f, .. }) if f == field)
        );
        assert_eq!(crate::lists::tests::snapshot(db.conn()), before);
        assert!(verify_folds(db.conn()).unwrap());
        assert!(counts::matches_recount(db.conn()));
    }
    record_activity(
        &mut db,
        &activity(id, ActivityKind::Memo, 0, None, NextAction::Set(0)),
    )
    .unwrap();
}

// REQ 10.3: WALの世代を固定し、主ファイルの増分も必要量に含める。
fn measured(db: &mut Db, label: &str, estimate: u64, f: impl FnOnce(&mut Db)) {
    db.conn()
        .execute_batch("PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let page_size: u64 = db
        .conn()
        .pragma_query_value(None, "page_size", |r| r.get::<_, u32>(0).map(u64::from))
        .unwrap();
    let before: u64 = db
        .conn()
        .pragma_query_value(None, "page_count", |r| r.get::<_, u32>(0).map(u64::from))
        .unwrap();
    f(db);
    let frames: u64 = db
        .conn()
        .query_row("PRAGMA wal_checkpoint(NOOP)", [], |r| {
            r.get::<_, u32>(1).map(u64::from)
        })
        .unwrap();
    db.conn()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let after: u64 = db
        .conn()
        .pragma_query_value(None, "page_count", |r| r.get::<_, u32>(0).map(u64::from))
        .unwrap();
    let actual = 32 + frames * (page_size + 24) + after.saturating_sub(before) * page_size;
    eprintln!("SPACE | {label} | {estimate} | {actual}");
    assert!(estimate >= actual, "{label}: {estimate} < {actual}");
    if label == "bulk 20000" {
        eprintln!(
            "SPACE bulk per company: estimate={}, actual={}",
            f64::from(u32::try_from(estimate).unwrap()) / 20000.0,
            f64::from(u32::try_from(actual).unwrap()) / 20000.0
        );
    }
}

fn estimate_database() -> (TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::create(
        &dir.path().join("estimates.db"),
        DataOrigin::Synthetic,
        &OpenOptions {
            background_checkpoint: false,
            ..Default::default()
        },
    )
    .unwrap();
    (dir, db)
}

// REQ 10.3
#[test]
fn estimates_cover_company_contact_and_activity_writes() {
    let (_dir, mut db) = estimate_database();
    let mut c = company();
    measured(&mut db, "register normal", company_estimate(&c), |db| {
        register_company(db, &c).unwrap();
    });
    c.name = "\u{fdfa}".repeat(5000);
    let mut id = CompanyId(0);
    measured(&mut db, "register NFKC", company_estimate(&c), |db| {
        id = register_company(db, &c).unwrap();
    });
    let old = normalize::name_key(&c.name);
    c.name.push('A');
    measured(
        &mut db,
        "rename to NFKC",
        update_company_estimate(&c, &old),
        |db| {
            update_company(db, id, &c).unwrap();
        },
    );
    let old = normalize::name_key(&c.name);
    c.name = "normal".into();
    measured(
        &mut db,
        "rename old NFKC",
        update_company_estimate(&c, &old),
        |db| {
            update_company(db, id, &c).unwrap();
        },
    );
    c.website = Some("w".repeat(1024 * 1024));
    c.source_id = Some("s".repeat(1024 * 1024));
    measured(
        &mut db,
        "register long fields",
        company_estimate(&c),
        |db| {
            register_company(db, &c).unwrap();
        },
    );
    // municipalityは5桁のみ有効。見積り関数自体が長い入力も数えることを確認する。
    let before = company_estimate(&c);
    c.municipality = Some("1".repeat(1024 * 1024));
    assert!(company_estimate(&c) >= before + 8 * (1024 * 1024 - 5));
    assert!(validate_company_input(&c).is_err());
    let contact = ContactInput {
        company: id,
        name: "氏名".repeat(200_000),
        phone: None,
        email: None,
    };
    measured(
        &mut db,
        "contact long name",
        contact_estimate(&contact),
        |db| {
            add_contact(db, &contact).unwrap();
        },
    );
    let mut a = activity(id, ActivityKind::Memo, 1, None, NextAction::Keep);
    a.note = Some("memo".repeat(300_000));
    measured(&mut db, "activity long memo", activity_estimate(&a), |db| {
        record_activity(db, &a).unwrap();
    });
}

// REQ 10.3
#[test]
fn estimates_cover_large_lists() {
    use crate::{
        lists,
        query::select_ids_sql,
        types::{BulkChange, Filters, OwnerChange},
    };
    let (_dir, mut db) = estimate_database();
    lists::tests::seed(&mut db, 20_000);
    let (sql, args) = select_ids_sql(&Filters::default(), 20_000).unwrap();
    let name = "L".repeat(1024 * 1024);
    let mut id = crate::types::ListId(0);
    measured(
        &mut db,
        "create list 1MiB",
        lists::create_list_estimate(&name, 20_000),
        |db| {
            id = lists::create_list_from_select(db, &name, &sql, &args, 20_000).unwrap();
        },
    );
    let change = BulkChange {
        owner: OwnerChange::Set(UserId(99)),
        status: Some(Status::Won),
        include_excluded: false,
    };
    let estimate =
        lists::bulk_update_estimate(20_000, lists::average_company_row_bytes(&db).unwrap());
    measured(&mut db, "bulk 20000", estimate, |db| {
        assert_eq!(lists::bulk_update(db, id, &change).unwrap(), 20_000);
    });
    measured(
        &mut db,
        "delete list 20000",
        lists::delete_list_estimate(20_000),
        |db| {
            lists::delete_list(db, id).unwrap();
        },
    );
}

fn notes(db: &Db, id: ContactId) {
    db.conn().execute("WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<5000) INSERT INTO activities(company_id,contact_id,kind,at,note) SELECT 1,?1,4,i,printf('%01024d',i) FROM seq", [id.0]).unwrap();
}

fn erasure_fixture(db: &mut Db) -> ContactId {
    crate::lists::tests::seed(db, 1);
    let id = add_contact(
        db,
        &ContactInput {
            company: CompanyId(1),
            name: "person".into(),
            phone: None,
            email: None,
        },
    )
    .unwrap();
    notes(db, id);
    id
}

// REQ 10.3
#[test]
fn estimates_cover_erasing_five_thousand_notes() {
    use crate::erase;
    let (_dir, mut db) = estimate_database();
    let id = erasure_fixture(&mut db);
    let page: u64 = db
        .conn()
        .pragma_query_value(None, "page_size", |r| r.get::<_, u32>(0).map(u64::from))
        .unwrap();
    measured(
        &mut db,
        "erase contact 5000",
        erase::erase_contact_estimate(page, 5000),
        |db| {
            erase::erase_contact(db, id, true).unwrap();
        },
    );
    let (_company_dir, mut db) = estimate_database();
    erasure_fixture(&mut db);
    measured(
        &mut db,
        "erase company 5000",
        erase::erase_company_estimate(page, 5000, 1, "synthetic1"),
        |db| {
            erase::erase_company(db, CompanyId(1)).unwrap();
        },
    );
}

// REQ 10.3: 件数取得は主キーと企業別の索引に限定する。
#[test]
fn estimate_count_queries_use_indexes() {
    let (_dir, db) = estimate_database();
    for sql in [
        "SELECT count(*) FROM activities WHERE company_id=1",
        "SELECT count(*) FROM contacts WHERE company_id=1",
        "SELECT company_id FROM contacts WHERE id=1",
        "SELECT name_key FROM companies WHERE id=1",
        "SELECT count(*) FROM list_members WHERE list_id=1",
    ] {
        let details = db
            .conn()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            details.iter().any(|s| s.contains("SEARCH")),
            "{sql}: {details:?}"
        );
        assert!(
            !details.iter().any(|s| s.starts_with("SCAN")),
            "{sql}: {details:?}"
        );
    }
}

struct EstimateSpace(std::sync::atomic::AtomicU64);
impl crate::schema::SpaceProbe for EstimateSpace {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

// REQ 10.3
#[test]
fn new_estimates_reject_before_any_contents_change() {
    use crate::{erase, lists, query::select_ids_sql, types::Filters};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(EstimateSpace(AtomicU64::new(u64::MAX)));
    let opt = OpenOptions {
        background_checkpoint: false,
        space_probe: Some(probe.clone()),
        ..Default::default()
    };
    let mut db = Db::create(&dir.path().join("reject.db"), DataOrigin::Synthetic, &opt).unwrap();
    let contact = erasure_fixture(&mut db);
    let before = lists::tests::snapshot(db.conn());
    let mut c = company();
    c.name = "\u{fdfa}".repeat(5000);
    // 旧式は約1.17MB、新式は約3.8MB。
    probe.0.store(2 * WRITE_BASE, Ordering::SeqCst);
    assert!(matches!(
        register_company(&mut db, &c),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(lists::tests::snapshot(db.conn()), before);
    assert!(matches!(
        update_company(&mut db, CompanyId(1), &c),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(lists::tests::snapshot(db.conn()), before);
    let (sql, args) = select_ids_sql(&Filters::default(), 1).unwrap();
    assert!(matches!(
        lists::create_list_from_select(&mut db, &"L".repeat(1024 * 1024), &sql, &args, 1),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(lists::tests::snapshot(db.conn()), before);
    assert!(matches!(
        erase::erase_contact(&mut db, contact, true),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(lists::tests::snapshot(db.conn()), before);
    // 旧キーだけでも拒否されることを確認する。
    probe.0.store(u64::MAX, Ordering::SeqCst);
    update_company(&mut db, CompanyId(1), &c).unwrap();
    let before = lists::tests::snapshot(db.conn());
    c.name = "short".into();
    probe.0.store(2 * WRITE_BASE, Ordering::SeqCst);
    assert!(matches!(
        update_company(&mut db, CompanyId(1), &c),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(lists::tests::snapshot(db.conn()), before);
}

// REQ 10.3
#[test]
fn estimates_cover_existing_large_company_rows_and_reject_without_changes() {
    use crate::lists::tests::snapshot;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(EstimateSpace(AtomicU64::new(u64::MAX)));
    let opt = OpenOptions {
        background_checkpoint: false,
        space_probe: Some(probe.clone()),
        ..Default::default()
    };
    let mut db = Db::create(
        &dir.path().join("large-row.db"),
        DataOrigin::Synthetic,
        &opt,
    )
    .unwrap();
    let mut c = company();
    c.website = Some("w".repeat(4 * 1024 * 1024));
    let id = register_company(&mut db, &c).unwrap();
    c.status = Status::Excluded;
    let excluded = register_company(&mut db, &c).unwrap();
    let a = activity(
        id,
        ActivityKind::Call,
        100,
        Some(Status::Won),
        NextAction::Set(500),
    );
    let row_bytes = company_row_bytes(db.conn(), id).unwrap();
    let normalized = validate_company_input(&c).unwrap();
    let expected_bytes = normalized.company.name.len()
        + normalized.name_key.len()
        + c.website.as_ref().unwrap().len()
        + c.source_id.as_ref().unwrap().len()
        + c.source_as_of.as_ref().unwrap().len()
        + c.municipality.as_ref().unwrap().len();
    assert_eq!(row_bytes, expected_bytes as u64);
    assert_eq!(company_row_bytes(db.conn(), CompanyId(999)).unwrap(), 0);
    let activity_estimate = activity_estimate(&a).saturating_add(row_bytes.saturating_mul(4));
    let release_estimate = WRITE_BASE.saturating_add(
        company_row_bytes(db.conn(), excluded)
            .unwrap()
            .saturating_mul(4),
    );
    assert!(activity_estimate > 17 * WRITE_BASE);
    let before = snapshot(db.conn());
    probe.0.store(2 * WRITE_BASE, Ordering::SeqCst);
    assert!(matches!(
        record_activity(&mut db, &a),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(snapshot(db.conn()), before);
    assert!(matches!(
        release_exclusion(&mut db, excluded, Status::Won),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(snapshot(db.conn()), before);
    probe.0.store(u64::MAX, Ordering::SeqCst);
    measured(
        &mut db,
        "activity existing 4MiB website",
        activity_estimate,
        |db| {
            record_activity(db, &a).unwrap();
        },
    );
    measured(
        &mut db,
        "release existing 4MiB website",
        release_estimate,
        |db| {
            release_exclusion(db, excluded, Status::Won).unwrap();
        },
    );
    assert_saved(&db, id, Status::Won, 100, Some(500));
    assert_eq!(saved(&db, excluded).status, Status::Won);
}

// REQ 10.3
#[test]
fn bulk_estimates_scale_with_database_average_and_keep_measured_floors() {
    use crate::lists::{self, average_company_row_bytes, bulk_update_estimate};
    let (_dir, mut db) = estimate_database();
    assert!(average_company_row_bytes(&db).unwrap() > 0);
    lists::tests::seed(&mut db, 2000);
    let small = average_company_row_bytes(&db).unwrap();
    assert!(small < 4096 / 3);
    assert_eq!(bulk_update_estimate(2000, small), WRITE_BASE + 2000 * 4096);
    db.conn()
        .execute("UPDATE companies SET website=?1", ["w".repeat(100 * 1024)])
        .unwrap();
    let average = average_company_row_bytes(&db).unwrap();
    assert!(average >= 100 * 1024);
    let estimate = bulk_update_estimate(2000, average);
    assert_eq!((estimate - WRITE_BASE) / 2000, 3 * average);
    let id = lists::create_list_from_select(
        &mut db,
        "large rows",
        "SELECT id FROM companies",
        &[],
        2000,
    )
    .unwrap();
    measured(
        &mut db,
        "bulk 2000 existing 100KiB websites",
        estimate,
        |db| {
            assert_eq!(
                lists::bulk_update(
                    db,
                    id,
                    &crate::types::BulkChange {
                        owner: crate::types::OwnerChange::Set(UserId(99)),
                        status: Some(Status::Won),
                        include_excluded: false,
                    }
                )
                .unwrap(),
                2000
            );
        },
    );
    assert!(bulk_update_estimate(100_000, 400) >= 100_000 * 2700);
    assert!(lists::delete_list_estimate(100_000) >= 100_000 * 16);
    assert_eq!(bulk_update_estimate(u64::MAX, u64::MAX), u64::MAX);
}
