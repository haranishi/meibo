use super::*;
use meibo_core::{
    Engine, OpenOptions,
    types::{DataOrigin, Filters},
};
use serde_json::json;

const PASSWORD: &str = "synthetic-test-password-only";
const AT: i64 = 1_800_000_000;

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::initialize(&temp.path().join("hosted")).unwrap();
    store.add_tenant("alpha", DataOrigin::Synthetic, 3).unwrap();
    store.add_tenant("beta", DataOrigin::Synthetic, 5).unwrap();
    store.add_user("operator", PASSWORD).unwrap();
    store
        .set_member("operator", "alpha", Some(Role::Admin))
        .unwrap();
    store
        .set_member("operator", "beta", Some(Role::Viewer))
        .unwrap();
    (temp, store)
}

fn login(store: &mut Store, tenant: &str) -> Login {
    match store.login(tenant, "operator", PASSWORD, AT).unwrap() {
        LoginOutcome::Success(login) => login,
        _ => panic!("synthetic login should succeed"),
    }
}

#[test]
fn passwords_are_salted_and_sessions_are_hashed_expire_and_revoke() {
    let (_temp, mut store) = fixture();
    store.add_user("second", PASSWORD).unwrap();
    let hashes: Vec<String> = store
        .conn
        .prepare("SELECT password_hash FROM users ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        hashes
            .iter()
            .all(|h| h.starts_with("$argon2id$") && !h.contains(PASSWORD))
    );
    assert_ne!(hashes[0], hashes[1]);
    let session = login(&mut store, "alpha");
    assert_eq!(session.token.len(), 64);
    let digest: Vec<u8> = store
        .conn
        .query_row("SELECT digest FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(digest.len(), 32);
    assert_ne!(digest, session.token.as_bytes());
    assert!(
        store
            .authenticate(&session.token, AT + 60)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .authenticate(&session.token, AT + 1861)
            .unwrap()
            .is_none()
    );
    let session = login(&mut store, "alpha");
    assert!(
        store
            .authenticate(&session.token, AT + 8 * 3600)
            .unwrap()
            .is_none()
    );
    let session = login(&mut store, "alpha");
    store.logout(&session.token).unwrap();
    assert!(
        store
            .authenticate(&session.token, AT + 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn membership_password_and_disable_changes_revoke_existing_sessions() {
    let (_temp, mut store) = fixture();
    let session = login(&mut store, "alpha");
    store
        .set_member("operator", "alpha", Some(Role::Editor))
        .unwrap();
    assert!(
        store
            .authenticate(&session.token, AT + 1)
            .unwrap()
            .is_none()
    );
    let session = login(&mut store, "alpha");
    store
        .set_password("operator", "a-different-test-password")
        .unwrap();
    assert!(
        store
            .authenticate(&session.token, AT + 1)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        store.login("alpha", "operator", PASSWORD, AT).unwrap(),
        LoginOutcome::Invalid
    ));
    store.disable_user("operator").unwrap();
    assert!(matches!(
        store
            .login("alpha", "operator", "a-different-test-password", AT)
            .unwrap(),
        LoginOutcome::Invalid
    ));
}

#[test]
fn failed_authentication_is_bounded_and_recovers_after_window() {
    let (_temp, mut store) = fixture();
    for _ in 0..5 {
        assert!(matches!(
            store
                .login("alpha", "operator", "incorrect-password", AT)
                .unwrap(),
            LoginOutcome::Invalid
        ));
    }
    assert!(matches!(
        store.login("alpha", "operator", PASSWORD, AT + 1).unwrap(),
        LoginOutcome::Throttled
    ));
    assert!(matches!(
        store
            .login("alpha", "operator", PASSWORD, AT + 901)
            .unwrap(),
        LoginOutcome::Success(_)
    ));
    assert!(
        store
            .add_tenant("../escape", DataOrigin::Synthetic, 0)
            .is_err()
    );
    assert!(store.add_user("secret", "short").is_err());
}

fn config() -> HostedConfig {
    HostedConfig::parse("https://crm.example.test", "127.0.0.1:4317", false).unwrap()
}

fn request(
    path: &str,
    method: &str,
    session: Option<&Login>,
    body: &serde_json::Value,
) -> crate::server::Input {
    let mut headers = vec![
        ("Host".into(), "crm.example.test".into()),
        ("Origin".into(), "https://crm.example.test".into()),
        ("X-Meibo-Local".into(), "1".into()),
        ("Content-Type".into(), "application/json".into()),
    ];
    if let Some(session) = session {
        headers.push((
            "Cookie".into(),
            format!("__Host-meibo-session={}", session.token),
        ));
        headers.push(("X-Meibo-Context".into(), session.identity.scope.clone()));
    }
    crate::server::Input {
        method: method.into(),
        target: path.into(),
        headers,
        body: if method == "POST" {
            body.to_string().into_bytes()
        } else {
            Vec::new()
        },
    }
}

#[test]
fn hosted_boundary_tenants_roles_context_and_csrf_are_enforced_before_dispatch() {
    let (_temp, store) = fixture();
    let mut app = Hosted::new(store, config());
    let alpha = login(&mut app.store, "alpha");
    let beta = login(&mut app.store, "beta");
    let body = |r: HostedReply| serde_json::from_slice::<serde_json::Value>(&r.reply.body).unwrap();
    assert_eq!(
        app.dispatch(&request("/api/summary", "GET", None, &json!(null)), AT)
            .reply
            .status,
        401
    );
    assert_eq!(
        body(app.dispatch(
            &request("/api/summary", "GET", Some(&alpha), &json!(null)),
            AT
        ))["total"],
        3
    );
    assert_eq!(
        body(app.dispatch(
            &request("/api/summary", "GET", Some(&beta), &json!(null)),
            AT
        ))["total"],
        5
    );
    assert_eq!(
        app.dispatch(
            &request(
                "/api/companies",
                "POST",
                Some(&beta),
                &json!({"name":"forbidden","prefecture":13})
            ),
            AT
        )
        .reply
        .status,
        403
    );
    let mut switched = request("/api/summary", "GET", Some(&beta), &json!(null));
    switched
        .headers
        .iter_mut()
        .find(|(k, _)| k == "X-Meibo-Context")
        .unwrap()
        .1 = alpha.identity.scope.clone();
    assert_eq!(app.dispatch(&switched, AT).reply.status, 409);
    for (header, bad) in [
        ("Host", "evil.example"),
        ("Origin", "https://evil.example"),
        ("X-Meibo-Local", "0"),
    ] {
        let mut r = request(
            "/api/companies",
            "POST",
            Some(&alpha),
            &json!({"name":"forbidden","prefecture":13}),
        );
        r.headers.iter_mut().find(|(k, _)| k == header).unwrap().1 = bad.into();
        assert_eq!(app.dispatch(&r, AT).reply.status, 403);
    }
    let html = app.dispatch(&request("/", "GET", Some(&alpha), &json!(null)), AT);
    let html = String::from_utf8(html.reply.body).unwrap();
    assert!(html.contains("data-meibo-src=\"/hosted-app.js\""));
    assert_eq!(html.matches("data-meibo-src=").count(), 1);
    assert_eq!(html.matches("rel=\"stylesheet\"").count(), 1);
    assert!(!html.contains("<script src=\"/hosted-app.js\""));
    assert!(html.contains("src=\"/login.js\""));
    for asset in ["/hosted-app.js", "/hosted-app.css"] {
        assert_eq!(
            app.dispatch(&request(asset, "GET", None, &json!(null)), AT)
                .reply
                .status,
            401
        );
        let reply = app.dispatch(&request(asset, "GET", Some(&alpha), &json!(null)), AT);
        assert_eq!(reply.reply.status, 200);
        assert!(reply.reply.body.len() > 1000);
    }
    assert!(HostedConfig::parse("http://crm.example.test", "0.0.0.0:4317", true).is_err());
    assert!(HostedConfig::parse("http://127.0.0.1:4317", "0.0.0.0:4317", true).is_err());
    assert!(HostedConfig::parse("http://127.0.0.1:4317", "127.0.0.1:4317", true).is_ok());
}

#[test]
fn backup_restore_preserves_both_tenants_and_rejects_tampering_and_overwrite() {
    let (temp, mut store) = fixture();
    let session = login(&mut store, "alpha");
    let snapshot = temp.path().join("snapshot");
    backup::create(&store, &snapshot).unwrap();
    let recovered = temp.path().join("recovered");
    backup::restore(&snapshot, &recovered).unwrap();
    let mut restored = Store::open(&recovered).unwrap();
    assert!(restored.authenticate(&session.token, AT).unwrap().is_none());
    assert!(matches!(
        restored.login("alpha", "operator", PASSWORD, AT).unwrap(),
        LoginOutcome::Success(_)
    ));
    for (tenant, count) in [("alpha", 3), ("beta", 5)] {
        let engine = Engine::open(
            &restored.tenant_path(tenant).unwrap(),
            &OpenOptions::default(),
        )
        .unwrap();
        assert_eq!(engine.count(&Filters::default()).unwrap().n, count);
        let report = engine.verify().unwrap();
        assert!(report.integrity_ok && report.counts_match && report.folds_match);
    }
    assert!(backup::restore(&snapshot, &recovered).is_err());
    assert!(backup::create(&store, &snapshot).is_err());
    std::fs::write(snapshot.join("tenants/alpha.sqlite3"), b"tampered").unwrap();
    assert!(backup::restore(&snapshot, &temp.path().join("tampered")).is_err());
}

#[test]
fn viewer_can_search_custom_fields_but_cannot_mutate_or_use_similar_paths() {
    use meibo_core::custom_fields::{CustomFieldInput, CustomFieldKind, CustomFieldValue};
    use meibo_core::types::CompanyId;
    let (_temp, store) = fixture();
    let mut engine =
        Engine::open(&store.tenant_path("beta").unwrap(), &OpenOptions::default()).unwrap();
    let field = engine
        .create_custom_field(&CustomFieldInput {
            name: "合成優先度".into(),
            kind: CustomFieldKind::Text,
            indexed: true,
        })
        .unwrap();
    engine
        .set_custom_value(CompanyId(1), field, &CustomFieldValue::Text("高".into()))
        .unwrap();
    drop(engine);
    let mut app = Hosted::new(store, config());
    let viewer = login(&mut app.store, "beta");
    let target = format!("/api/custom-fields/{field}/search");
    let result = app.dispatch(
        &request(&target, "POST", Some(&viewer), &json!({"value":"高"})),
        AT,
    );
    assert_eq!(result.reply.status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result.reply.body).unwrap()["company_ids"],
        json!([1])
    );
    for target in [
        format!("/api/custom-fields/{field}/delete"),
        "/api/custom-fields".into(),
        "/api/companies/1/custom-fields".into(),
        "/api/custom-fields/+1/search".into(),
        "/api/custom-fields/1/search/extra".into(),
    ] {
        assert_eq!(
            app.dispatch(&request(&target, "POST", Some(&viewer), &json!({})), AT)
                .reply
                .status,
            403
        );
    }
}
