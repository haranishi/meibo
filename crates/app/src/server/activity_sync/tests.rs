use super::*;
use meibo_core::{
    OpenOptions,
    crm::{WorkKind, WorkQuery},
    types::{CompanyInput, ContactInput, Status},
};
use rusqlite::Connection;

fn fixture() -> (
    tempfile::TempDir,
    Engine,
    Connection,
    meibo_core::types::ContactId,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &dir.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let company = engine
        .register_company(&CompanyInput {
            origin: DataOrigin::Synthetic,
            corporate_number: None,
            name: "合成企業".into(),
            prefecture: 13,
            municipality: None,
            industry: None,
            phone: None,
            website: None,
            owner: None,
            status: Status::Untouched,
            is_sole_proprietor: false,
            source_id: None,
            source_as_of: None,
        })
        .unwrap();
    let contact = engine
        .add_contact(&ContactInput {
            company,
            name: "合成担当".into(),
            phone: None,
            email: Some("Person@example.invalid".into()),
        })
        .unwrap();
    let private = tempfile::Builder::new()
        .prefix("private-")
        .tempdir_in(dir.path())
        .unwrap()
        .keep();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let conn = vault::open(&private).unwrap();
    (dir, engine, conn, contact)
}
fn worker(input: &Value) -> Value {
    match input["operation"].as_str().unwrap() {
        "begin" => {
            json!({"state":"a".repeat(64),"verifier":"synthetic-verifier","url":"https://example.invalid/test"})
        }
        "finish" => {
            json!({"refresh":"synthetic-initial","access":"short","account":"subject-private","address":"agent@example.invalid","expires":0,"initial":{"history":"before"}})
        }
        "refresh" => {
            json!({"refresh":"synthetic-rotated","access":"new-short","expires":(now()+3600)*1000})
        }
        _ => panic!("unexpected worker operation"),
    }
}
fn finish(conn: &mut Connection, engine: &mut Engine, scope: &str, provider: &str) {
    vault::apply(conn, engine, scope, provider, "begin", &json!({}), |a| {
        Ok(worker(&a))
    })
    .unwrap();
    vault::apply(
        conn,
        engine,
        scope,
        provider,
        "finish",
        &json!({"state":"a".repeat(64),"code":"code"}),
        |a| Ok(worker(&a)),
    )
    .unwrap();
}
fn mail() -> Value {
    json!({"id":"external-message-private","kind":"mail","subject":"合成メール","at":100,"addresses":["person@example.invalid"]})
}
fn event() -> Value {
    json!({"id":"external-event-private","kind":"event","subject":"合成予定","at":200,"end":300,"addresses":["PERSON@example.invalid"]})
}
fn work(engine: &Engine) -> Vec<meibo_core::crm::WorkItem> {
    engine
        .crm_work_items(&WorkQuery {
            company: None,
            kind: Some(WorkKind::Event),
            completed: None,
            after: None,
            limit: 20,
        })
        .unwrap()
        .items
}

#[test]
fn consent_is_scoped_one_time() {
    let (_dir, mut engine, mut conn, _) = fixture();
    let (a, b) = ("alpha/admin", "beta/admin");
    vault::apply(
        &mut conn,
        &mut engine,
        a,
        "google",
        "begin",
        &json!({}),
        |v| Ok(worker(&v)),
    )
    .unwrap();
    let body = json!({"state":"a".repeat(64),"code":"code"});
    for (owner, p) in [(b, "google"), (a, "microsoft")] {
        assert!(matches!(
            vault::apply(
                &mut conn,
                &mut engine,
                owner,
                p,
                "finish",
                &body,
                |_| panic!("untrusted state")
            ),
            Err(Failure::Stale)
        ));
    }
    vault::apply(&mut conn, &mut engine, a, "google", "finish", &body, |v| {
        Ok(worker(&v))
    })
    .unwrap();
    assert!(matches!(
        vault::apply(
            &mut conn,
            &mut engine,
            a,
            "google",
            "finish",
            &body,
            |_| panic!("replayed code")
        ),
        Err(Failure::Stale)
    ));
}

#[test]
fn rotation_survives_a_failed_page_and_disconnect_stops_the_session() {
    let (_dir, mut engine, mut conn, _) = fixture();
    let (a, b) = ("alpha/admin", "beta/admin");
    finish(&mut conn, &mut engine, a, "google");
    assert!(
        vault::apply(
            &mut conn,
            &mut engine,
            a,
            "google",
            "pull",
            &json!({}),
            |v| {
                if v["operation"] == "refresh" {
                    return Ok(worker(&v));
                }
                Err(Failure::Unavailable)
            }
        )
        .is_err()
    );
    let (cred, cs): (String, String) = conn
        .query_row("SELECT refresh,cursors FROM grants", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(cred, "synthetic-rotated");
    assert_eq!(
        serde_json::from_str::<Value>(&cs).unwrap()["inbox"]["history"],
        "before"
    );
    let next = vault::apply(
        &mut conn,
        &mut engine,
        a,
        "google",
        "pull",
        &json!({}),
        |v| {
            assert_eq!(v["access"], "new-short");
            assert_eq!(v["cursor"]["history"], "before");
            Ok(json!({"items":[mail()],"cursor":{"history":"after"},"more":false}))
        },
    )
    .unwrap();
    assert_eq!(next["changes"]["created"], 1);
    let status = vault::status(&conn, a, "google").unwrap().to_string();
    assert!(!status.contains("synthetic-rotated") && !status.contains("subject-private"));
    assert_eq!(
        vault::status(&conn, b, "google").unwrap()["connected"],
        false
    );
    vault::apply(
        &mut conn,
        &mut engine,
        a,
        "google",
        "disconnect",
        &json!({}),
        |_| panic!("network on disconnect"),
    )
    .unwrap();
    assert!(matches!(
        vault::apply(
            &mut conn,
            &mut engine,
            a,
            "google",
            "pull",
            &json!({}),
            |_| panic!("disconnected")
        ),
        Err(Failure::Stale)
    ));
}

#[test]
fn provider_links_match_only_unique_contacts_preserve_edits_and_erase_without_reimport() {
    let (dir, mut engine, _, person) = fixture();
    let records = vec![
        mail(),
        event(),
        json!({"kind":"mail","id":"unrelated","at":1,"subject":"private","addresses":["stranger@example.invalid"]}),
    ];
    let convert =
        |v: Value| serde_json::from_value::<Vec<meibo_core::activity_sync::Item>>(v).unwrap();
    for (created, skipped) in [(2, 1), (0, 3)] {
        let changes = engine
            .sync_activity_page("google", "subject-private", &convert(json!(records)))
            .unwrap();
        assert_eq!((changes.created, changes.skipped), (created, skipped));
    }
    let row = work(&engine).remove(0);
    let mut updated = event();
    updated["subject"] = json!("変更後");
    updated["at"] = json!(251);
    engine
        .sync_activity_page("google", "subject-private", &convert(json!([updated])))
        .unwrap();
    let row_after = work(&engine).remove(0);
    assert_eq!(row.id, row_after.id);
    assert_eq!(row_after.input.subject, "変更後");
    let cancelled = json!({"kind":"removed_event","id":"external-event-private"});
    assert_eq!(
        engine
            .sync_activity_page("google", "subject-private", &convert(json!([cancelled])))
            .unwrap()
            .updated,
        1
    );
    assert_eq!(work(&engine), []);
    engine
        .sync_activity_page("google", "subject-private", &convert(json!([event()])))
        .unwrap();
    assert_eq!(work(&engine).len(), 1);
    let twin = engine
        .add_contact(&ContactInput {
            company: row.input.company,
            name: "合成同名".into(),
            phone: None,
            email: Some("PERSON@example.invalid".into()),
        })
        .unwrap();
    let mut new_message = mail();
    new_message["id"] = json!("ambiguous");
    assert_eq!(
        engine
            .sync_activity_page("google", "subject-private", &convert(json!([new_message])))
            .unwrap()
            .skipped,
        1
    );
    engine.erase_contact(person, false).unwrap();
    assert_ne!(twin, person);
    assert_eq!(work(&engine), []);
    engine
        .sync_activity_page(
            "google",
            "subject-private",
            &convert(json!([event(), mail()])),
        )
        .unwrap();
    assert_eq!(work(&engine), []);
    let db = Connection::open(dir.path().join("synthetic.db")).unwrap();
    let note: Option<String> = db
        .query_row("SELECT note FROM activities WHERE kind=3", [], |r| r.get(0))
        .unwrap();
    assert!(note.is_none());
    let bad:u32=db.query_row("SELECT count(*) FROM crm_sync_links WHERE length(source_key)<>64 OR source_key GLOB '*[^0-9a-f]*'",[],|r|r.get(0)).unwrap();
    assert_eq!(bad, 0);
}

#[test]
fn baseline_after_a_closed_session_reconciles_disappeared_events_and_rejects_partial_pages() {
    let (_dir, mut engine, mut conn, _) = fixture();
    finish(&mut conn, &mut engine, "local", "microsoft");
    let pull = |db: &mut Connection, e: &mut Engine, output: &Value| {
        vault::apply(db, e, "local", "microsoft", "pull", &json!({}), |v| {
            if v["operation"] == "refresh" {
                Ok(worker(&v))
            } else {
                Ok(output.clone())
            }
        })
    };
    let empty = json!({"items":[],"more":false,"cursor":{"at":"updated"}});
    pull(&mut conn, &mut engine, &empty).unwrap();
    pull(&mut conn, &mut engine, &empty).unwrap();
    let first = json!({"items":[event()],"cursor":{"token":"source"},"more":false,"epoch":"first","reconcile":true});
    assert_eq!(pull(&mut conn, &mut engine, &first).unwrap()["more"], false);
    assert_eq!(work(&engine).len(), 1);
    // A second session opens the same vault after the browser has closed.
    let path = conn.path().unwrap().to_string();
    drop(conn);
    let mut conn = Connection::open(path).unwrap();
    let bad = json!({"items":[mail(),{"id":"bad","kind":"event","at":20,"end":5}],"cursor":{"skip":"bad"},"more":false});
    assert!(pull(&mut conn, &mut engine, &bad).is_err());
    assert!(
        !conn
            .query_row("SELECT cursors FROM grants", [], |r| r.get::<_, String>(0))
            .unwrap()
            .contains("bad")
    );
    pull(&mut conn, &mut engine, &empty).unwrap();
    pull(&mut conn, &mut engine, &empty).unwrap();
    let next =
        json!({"items":[],"cursor":{"token":"new"},"more":false,"epoch":"second","reconcile":true});
    assert_eq!(
        pull(&mut conn, &mut engine, &next).unwrap()["changes"]["updated"],
        1
    );
    assert_eq!(work(&engine), []);
}

#[test]
fn synthetic_workspaces_never_start_a_provider_and_public_callback_is_static() {
    let (_dir, mut engine, _vault, _) = fixture();
    let input = Input {
        method: "POST".into(),
        target: "/api/activity-sync/google/begin".into(),
        headers: vec![],
        body: b"{}".to_vec(),
    };
    assert!(matches!(
        route(&mut engine, &input, "/google/begin", "", Some("scope")),
        Err(Failure::Setup)
    ));
    assert!(callback("/api/activity-sync/google/begin").is_none());
    assert!(
        !String::from_utf8(callback("/oauth/activity-return").unwrap().body)
            .unwrap()
            .contains("secret")
    );
}

#[test]
fn a_different_database_at_the_same_local_path_requires_new_consent() {
    let (dir, mut engine, _, _) = fixture();
    assert!(scope(&mut engine, None, false).unwrap().is_none());
    let original = scope(&mut engine, None, true).unwrap().unwrap();
    assert_eq!(
        scope(&mut engine, None, false).unwrap().as_ref(),
        Some(&original)
    );
    drop(engine);
    let path = dir.path().join("synthetic.db");
    std::fs::rename(&path, dir.path().join("original.db")).unwrap();
    let mut replacement =
        Engine::create(&path, DataOrigin::Synthetic, &OpenOptions::default()).unwrap();
    assert!(scope(&mut replacement, None, false).unwrap().is_none());
    assert_ne!(
        scope(&mut replacement, None, true).unwrap().unwrap(),
        original
    );
}
