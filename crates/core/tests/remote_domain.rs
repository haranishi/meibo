//! Shared domain SQL on real, explicitly enabled synthetic remote databases.

#![cfg(feature = "integration-tests")]

use std::{
    io,
    sync::{Arc, Mutex},
};

use meibo_core::{
    Engine, OpenOptions, RemoteIdentity, SpaceProbe,
    crm::{OpportunityInput, OpportunityQuery, WorkItemInput, WorkKind, WorkQuery},
    custom_fields::{CustomFieldInput, CustomFieldKind, CustomFieldSearch, CustomFieldValue},
    public_data::PublicCompanyInput,
    types::*,
};
use meibo_db::{Connection, RemoteErrorKind, RemoteOptions, remote_error_kind};

#[allow(dead_code)]
#[path = "../../db/tests/support/client.rs"]
mod client;
#[allow(dead_code)]
#[path = "../../db/tests/support/proxy.rs"]
mod proxy;
#[allow(dead_code)]
#[path = "../../db/tests/support/sqld.rs"]
mod sqld;
#[allow(dead_code)]
#[path = "engine/support.rs"]
mod support;

static FIXTURES: Mutex<()> = Mutex::new(());

struct NoFilesystem;
impl SpaceProbe for NoFilesystem {
    fn available(&self, _: &std::path::Path) -> io::Result<u64> {
        panic!("remote domain operations must not inspect a local filesystem")
    }
}

fn options() -> OpenOptions {
    OpenOptions {
        space_probe: Some(Arc::new(NoFilesystem)),
        background_checkpoint: true,
        ..OpenOptions::default()
    }
}

fn endpoint(server: &sqld::Sqld) -> RemoteOptions {
    RemoteOptions::new(server.endpoint(), "synthetic-domain-token").allow_loopback_http_for_tests()
}

fn identity() -> RemoteIdentity {
    RemoteIdentity::new("tenant-alpha", "crm").unwrap()
}

fn with_database(origin: DataOrigin, run: impl FnOnce(Engine, &Connection, &sqld::Sqld)) {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = sqld::Sqld::start().unwrap();
    let conn = Connection::open_remote(endpoint(&server)).unwrap();
    let engine = Engine::create_remote(endpoint(&server), &identity(), origin, &options()).unwrap();
    run(engine, &conn, &server);
}

fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn assert_counts(conn: &Connection) {
    assert_eq!(
        scalar(conn, "SELECT coalesce(sum(n),0) FROM counts"),
        scalar(conn, "SELECT count(*) FROM companies")
    );
    assert_eq!(
        scalar(
            conn,
            "SELECT count(*) FROM counts c WHERE c.n<>(SELECT count(*) FROM companies b WHERE b.status=c.status AND b.has_phone=c.has_phone)"
        ),
        0
    );
    assert_eq!(scalar(conn, "SELECT count(*) FROM erase_gate"), 0);
}

fn text_field(engine: &mut Engine) -> i64 {
    engine
        .create_custom_field(&CustomFieldInput {
            name: "合成内部メモ".into(),
            kind: CustomFieldKind::Text,
            indexed: true,
        })
        .unwrap()
}

fn task(company: CompanyId, contact: Option<ContactId>) -> WorkItemInput {
    WorkItemInput {
        company,
        contact,
        kind: WorkKind::Task,
        subject: "合成フォロー".into(),
        due_at: Some(1000),
        start_at: None,
        end_at: None,
        completed: false,
    }
}

fn opportunity(company: CompanyId, contact: Option<ContactId>) -> OpportunityInput {
    OpportunityInput {
        company,
        contact,
        name: "合成案件".into(),
        amount_jpy: 300_000,
        stage: 0,
        close_date: "2026-12-01".into(),
    }
}

fn work_query(company: CompanyId) -> WorkQuery {
    WorkQuery {
        company: Some(company),
        kind: None,
        completed: None,
        after: None,
        limit: 200,
    }
}

fn opportunity_query(company: CompanyId) -> OpportunityQuery {
    OpportunityQuery {
        company: Some(company),
        after: None,
        limit: 200,
    }
}

#[test]
fn crud_activity_cancellation_folds_counts_and_two_and_three_character_search() {
    with_database(DataOrigin::Synthetic, |mut engine, conn, server| {
        let mut company = support::company();
        company.name = "合成星空会社".into();
        let id = engine.register_company(&company).unwrap();
        let mut person = ContactInput {
            company: id,
            name: "合成担当".into(),
            phone: None,
            email: None,
        };
        let contact = engine.add_contact(&person).unwrap();
        person.name = "更新合成担当".into();
        person.email = Some("synthetic@example.invalid".into());
        engine.update_contact(contact, &person).unwrap();
        company.name = "合成青空会社".into();
        company.phone = None;
        engine.update_company(id, &company).unwrap();
        assert_eq!(
            engine.search_name("星空", &support::search()).unwrap().rows,
            []
        );
        for term in ["青空", "青空会"] {
            assert_eq!(
                engine.search_name(term, &support::search()).unwrap().rows[0].id,
                id
            );
        }
        let mut call = support::activity(id);
        call.kind = ActivityKind::Call;
        call.at = 100;
        call.contact = Some(contact);
        call.status_after = Some(Status::InProgress);
        call.next_action = NextAction::Set(500);
        let call_id = engine.record_activity(&call).unwrap();
        let state = engine.company_detail(id, &support::fields()).unwrap();
        assert_eq!(
            (
                state.company.status,
                state.company.last_activity_at,
                state.company.next_action_at
            ),
            (Status::InProgress, 100, Some(500))
        );
        assert_eq!(
            state.contacts[0].name.as_deref(),
            Some(person.name.as_str())
        );
        engine
            .record_activity(&ActivityInput {
                kind: ActivityKind::Cancel,
                reverses: Some(call_id),
                ..support::activity(id)
            })
            .unwrap();
        let state = engine.company_detail(id, &support::fields()).unwrap();
        assert_eq!(
            (
                state.company.status,
                state.company.last_activity_at,
                state.company.next_action_at
            ),
            (Status::Untouched, 0, None)
        );
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 1);
        let excluded = engine
            .register_company(&CompanyInput {
                status: Status::Excluded,
                ..support::company()
            })
            .unwrap();
        engine
            .release_exclusion(excluded, Status::Negotiating)
            .unwrap();
        assert_eq!(
            engine
                .count(&Filters {
                    status: vec![Status::Negotiating],
                    ..Filters::default()
                })
                .unwrap()
                .n,
            1
        );
        assert_counts(conn);
        drop(engine);
        let reopened = Engine::open_remote(endpoint(server), &identity(), &options()).unwrap();
        assert_eq!(
            reopened.company_detail(id, &support::fields()).unwrap(),
            state
        );
    });
}

#[test]
fn list_membership_and_bulk_owner_status_changes_share_counts_and_activities() {
    with_database(DataOrigin::Synthetic, |mut engine, conn, _| {
        let a = engine.register_company(&support::company()).unwrap();
        let b = engine.register_company(&support::company()).unwrap();
        let list = engine
            .create_list("合成固定リスト", &Filters::default(), 100)
            .unwrap();
        assert_eq!(scalar(conn, "SELECT count(*) FROM list_members"), 2);
        assert_eq!(
            engine
                .bulk_update(
                    list,
                    &BulkChange {
                        owner: OwnerChange::Set(UserId(7)),
                        status: Some(Status::Negotiating),
                        include_excluded: false
                    }
                )
                .unwrap(),
            2
        );
        assert_eq!(
            engine
                .count(&Filters {
                    status: vec![Status::Negotiating],
                    ..Filters::default()
                })
                .unwrap()
                .n,
            2
        );
        for id in [a, b] {
            let detail = engine.company_detail(id, &support::fields()).unwrap();
            assert_eq!(detail.company.owner, Some(UserId(7)));
            assert_eq!(detail.activities.len(), 1);
            assert_eq!(detail.activities[0].kind, ActivityKind::StatusChange);
        }
        engine
            .bulk_update(
                list,
                &BulkChange {
                    owner: OwnerChange::Clear,
                    status: None,
                    include_excluded: false,
                },
            )
            .unwrap();
        engine.delete_list(list).unwrap();
        assert_eq!(scalar(conn, "SELECT count(*) FROM list_members"), 0);
        assert_counts(conn);
    });
}

#[test]
fn custom_fields_work_and_opportunities_update_search_clear_delete_and_reopen() {
    with_database(DataOrigin::Synthetic, |mut engine, conn, server| {
        let id = engine.register_company(&support::company()).unwrap();
        let field = text_field(&mut engine);
        let value = CustomFieldValue::Text("合成顧客情報".into());
        engine.set_custom_value(id, field, &value).unwrap();
        let search = CustomFieldSearch {
            field_id: field,
            value: value.clone(),
            after: None,
            limit: 100,
        };
        assert_eq!(
            engine.search_custom_field(&search).unwrap().company_ids,
            vec![id]
        );
        engine.clear_custom_value(id, field).unwrap();
        assert_eq!(engine.search_custom_field(&search).unwrap().company_ids, []);
        engine.set_custom_value(id, field, &value).unwrap();
        engine.delete_custom_field(field).unwrap();
        assert_eq!(engine.custom_values(id).unwrap(), []);
        let work = engine.crm_create_work_item(&task(id, None)).unwrap();
        engine
            .crm_create_work_item(&WorkItemInput {
                kind: WorkKind::Event,
                due_at: None,
                start_at: Some(2000),
                end_at: Some(3000),
                ..task(id, None)
            })
            .unwrap();
        let mut update = task(id, None);
        update.subject = "合成フォロー変更".into();
        engine.crm_update_work_item(work, &update).unwrap();
        engine.crm_set_completion(work, true).unwrap();
        let deal = engine
            .crm_create_opportunity(&opportunity(id, None))
            .unwrap();
        let mut update = opportunity(id, None);
        update.amount_jpy = 400_000;
        engine.crm_update_opportunity(deal, &update).unwrap();
        engine.crm_set_stage(deal, 4).unwrap();
        drop(engine);
        let reopened = Engine::open_remote(endpoint(server), &identity(), &options()).unwrap();
        let works = reopened.crm_work_items(&work_query(id)).unwrap();
        assert_eq!(works.total, 2);
        assert_eq!(works.items[1].input.kind, WorkKind::Event);
        assert_eq!(works.items[1].input.start_at, Some(2000));
        assert_eq!(works.items[1].input.end_at, Some(3000));
        assert_eq!(
            (
                works.items[0].input.subject.as_str(),
                works.items[0].input.completed
            ),
            ("合成フォロー変更", true)
        );
        let deals = reopened.crm_opportunities(&opportunity_query(id)).unwrap();
        assert_eq!(
            (deals.items[0].input.amount_jpy, deals.items[0].input.stage),
            (400_000, 4)
        );
        assert_counts(conn);
    });
}

#[test]
fn imports_reject_whole_duplicate_batch_and_load_synthetic_without_file_policy() {
    with_database(DataOrigin::Synthetic, |mut engine, conn, _| {
        let company = CompanyInput {
            corporate_number: Some(meibo_core::houjin::generate(800).unwrap()),
            ..support::company()
        };
        let report = engine
            .import_companies(&[company.clone(), company.clone()], DuplicatePolicy::Skip)
            .unwrap();
        assert_eq!((report.inserted, report.skipped), (1, 1));
        let new = CompanyInput {
            name: "未確定合成社".into(),
            corporate_number: None,
            ..support::company()
        };
        let failure = engine
            .import_companies(&[new, company], DuplicatePolicy::Error)
            .unwrap_err();
        assert_eq!(failure.row, 2);
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 1);
        assert_eq!(
            engine
                .search_name("未確定", &support::search())
                .unwrap()
                .rows,
            []
        );
        let report = engine
            .load_synthetic(support::config(3, meibo_core::synth::Profile::Dense))
            .unwrap();
        assert_eq!(report.companies, 3);
        assert_eq!(scalar(conn, "SELECT count(*) FROM companies"), 4);
        assert_counts(conn);
    });
}

#[test]
fn real_trigger_failure_rolls_back_company_fts_and_activity_count_changes() {
    with_database(DataOrigin::Synthetic, |mut engine, conn, _| {
        let company = support::company();
        let id = engine.register_company(&company).unwrap();
        let before = engine.company_detail(id, &support::fields()).unwrap();
        conn.execute_batch("CREATE TRIGGER injected_update BEFORE UPDATE OF name ON companies BEGIN SELECT RAISE(ABORT,'synthetic injected update'); END;").unwrap();
        let renamed = CompanyInput {
            name: "変更予定会社".into(),
            phone: None,
            ..company
        };
        assert!(engine.update_company(id, &renamed).is_err());
        assert_eq!(
            engine.company_detail(id, &support::fields()).unwrap(),
            before
        );
        assert_eq!(
            engine
                .search_name("変更予定", &support::search())
                .unwrap()
                .rows,
            []
        );
        assert_eq!(
            engine
                .search_name("合成会", &support::search())
                .unwrap()
                .rows
                .len(),
            1
        );
        conn.execute_batch("DROP TRIGGER injected_update;CREATE TRIGGER injected_activity BEFORE UPDATE OF status ON companies BEGIN SELECT RAISE(ABORT,'synthetic injected activity'); END;").unwrap();
        assert!(
            engine
                .record_activity(&ActivityInput {
                    kind: ActivityKind::StatusChange,
                    status_after: Some(Status::Negotiating),
                    ..support::activity(id)
                })
                .is_err()
        );
        assert_eq!(
            engine.company_detail(id, &support::fields()).unwrap(),
            before
        );
        assert_eq!(scalar(conn, "SELECT count(*) FROM activities"), 0);
        let list = engine
            .create_list("合成取消確認", &Filters::default(), 100)
            .unwrap();
        assert!(
            engine
                .bulk_update(
                    list,
                    &BulkChange {
                        owner: OwnerChange::Set(UserId(9)),
                        status: Some(Status::Negotiating),
                        include_excluded: false
                    }
                )
                .is_err()
        );
        assert_eq!(
            engine.company_detail(id, &support::fields()).unwrap(),
            before
        );
        assert_eq!(scalar(conn, "SELECT count(*) FROM activities"), 0);
        assert_counts(conn);
        assert!(!engine.is_poisoned());
    });
}

fn assert_failed_company_erase(engine: &mut Engine, conn: &Connection, id: CompanyId) {
    let before = engine.company_detail(id, &support::fields()).unwrap();
    conn.execute_batch("CREATE TRIGGER injected_erase BEFORE UPDATE OF erased_at ON companies BEGIN SELECT RAISE(ABORT,'synthetic injected erase'); END;").unwrap();
    assert!(engine.erase_company(id).is_err());
    assert_eq!(
        engine.company_detail(id, &support::fields()).unwrap(),
        before
    );
    assert_eq!(engine.custom_values(id).unwrap().len(), 1);
    assert_eq!(
        scalar(
            conn,
            "SELECT count(*) FROM crm_work_items WHERE subject<>''"
        ),
        1
    );
    assert_counts(conn);
    conn.execute_batch("DROP TRIGGER injected_erase;").unwrap();
}

#[test]
fn logical_contact_and_company_erasure_redact_linked_data_and_clear_the_gate() {
    with_database(DataOrigin::Synthetic, |mut engine, conn, _| {
        let id = engine
            .register_company(&CompanyInput {
                name: "合成消去対象事業".into(),
                is_sole_proprietor: true,
                ..support::company()
            })
            .unwrap();
        let contact = engine
            .add_contact(&ContactInput {
                company: id,
                name: "合成個人".into(),
                phone: Some("0123".into()),
                email: Some("synthetic@example.invalid".into()),
            })
            .unwrap();
        engine
            .record_activity(&ActivityInput {
                contact: Some(contact),
                ..support::activity(id)
            })
            .unwrap();
        engine
            .crm_create_work_item(&task(id, Some(contact)))
            .unwrap();
        engine
            .crm_create_opportunity(&opportunity(id, Some(contact)))
            .unwrap();
        engine.erase_contact(contact, true).unwrap();
        let detail = engine.company_detail(id, &support::fields()).unwrap();
        assert!(
            detail.contacts[0].name.is_none()
                && detail.contacts[0].phone.is_none()
                && detail.contacts[0].email.is_none()
        );
        assert!(detail.activities[0].note.is_none());
        assert_eq!(
            scalar(
                conn,
                "SELECT count(*) FROM crm_work_items WHERE contact_id IS NOT NULL OR subject<>''"
            ),
            0
        );
        assert_eq!(
            scalar(
                conn,
                "SELECT count(*) FROM crm_opportunities WHERE contact_id IS NOT NULL OR name<>''"
            ),
            0
        );
        let field = text_field(&mut engine);
        engine
            .set_custom_value(
                id,
                field,
                &CustomFieldValue::Text("合成内部個人情報".into()),
            )
            .unwrap();
        engine.crm_create_work_item(&task(id, None)).unwrap();
        engine
            .crm_create_opportunity(&opportunity(id, None))
            .unwrap();
        assert_failed_company_erase(&mut engine, conn, id);
        engine.erase_company(id).unwrap();
        engine.erase_company(id).unwrap();
        assert_eq!(
            engine
                .search_name("消去対象", &support::search())
                .unwrap()
                .rows,
            []
        );
        assert!(matches!(
            engine.custom_values(id),
            Err(EngineError::NotFound)
        ));
        assert_eq!(scalar(conn, "SELECT count(*) FROM custom_field_values"), 0);
        assert_eq!(
            scalar(
                conn,
                "SELECT count(*) FROM crm_work_items WHERE subject<>''"
            ),
            0
        );
        assert_eq!(
            scalar(
                conn,
                "SELECT count(*) FROM crm_opportunities WHERE name<>''"
            ),
            0
        );
        assert_eq!(
            scalar(
                conn,
                "SELECT count(*) FROM companies WHERE erased_at IS NOT NULL AND status=5 AND phone IS NULL AND owner_id IS NULL AND corporate_number IS NULL"
            ),
            1
        );
        assert_counts(conn);
    });
}

#[test]
fn company_erasure_tombstone_prevents_public_reimport() {
    with_database(DataOrigin::Real, |mut engine, conn, _| {
        let number = meibo_core::houjin::generate(900).unwrap();
        let company = CompanyInput {
            origin: DataOrigin::Real,
            corporate_number: Some(number.clone()),
            is_sole_proprietor: true,
            ..support::company()
        };
        let id = engine.register_company(&company).unwrap();
        engine.erase_company(id).unwrap();
        assert_eq!(
            scalar(
                conn,
                "SELECT count(*) FROM erased_corporate_numbers WHERE length(number_hash)=32"
            ),
            1
        );
        let report = engine
            .upsert_public_companies(&[PublicCompanyInput {
                corporate_number: number,
                name: "再取込合成社".into(),
                prefecture: 13,
                source_as_of: "2026-10-27".into(),
            }])
            .unwrap();
        assert_eq!((report.inserted, report.updated, report.skipped), (0, 0, 1));
        assert_eq!(scalar(conn, "SELECT count(*) FROM companies"), 1);
        assert_counts(conn);
    });
}

fn remote_kind(error: &EngineError) -> Option<RemoteErrorKind> {
    match error {
        EngineError::Storage(source) => source
            .downcast_ref::<meibo_db::Error>()
            .and_then(remote_error_kind),
        _ => None,
    }
}

#[test]
fn lost_commit_response_preserves_unknown_outcome_and_poison_without_cleanup_or_retry() {
    with_database(DataOrigin::Synthetic, |engine, conn, server| {
        drop(engine);
        let proxy = proxy::FaultProxy::start(server.address()).unwrap();
        let remote = RemoteOptions::new(proxy.endpoint(), "synthetic-domain-token")
            .allow_loopback_http_for_tests();
        let mut engine = Engine::open_remote(remote, &identity(), &options()).unwrap();
        let commits = proxy.commits();
        let rollbacks = proxy.rollbacks();
        proxy.arm(proxy::Fault::LoseCommitResponse);
        let error = engine.register_company(&support::company()).unwrap_err();
        assert_eq!(remote_kind(&error), Some(RemoteErrorKind::OutcomeUnknown));
        assert!(engine.is_poisoned());
        assert_eq!(proxy.triggered(), 1);
        assert_eq!(proxy.commits(), commits + 1);
        assert_eq!(proxy.rollbacks(), rollbacks);
        assert_eq!(scalar(conn, "SELECT count(*) FROM companies"), 1);
        assert_counts(conn);
        let requests = proxy.requests();
        assert_eq!(
            remote_kind(&engine.register_company(&support::company()).unwrap_err()),
            Some(RemoteErrorKind::ConnectionPoisoned)
        );
        assert_eq!(
            remote_kind(&engine.count(&Filters::default()).unwrap_err()),
            Some(RemoteErrorKind::ConnectionPoisoned)
        );
        drop(engine);
        assert_eq!(proxy.requests(), requests);
    });
}
