use super::*;
use crate::hosted::access_tests as proof;
use meibo_core::types::DataOrigin;
use std::io::Write;

const PASSWORD: &str = "synthetic-test-password-only";
struct Session {
    cookie: String,
    scope: String,
}

fn fixture() -> (tempfile::TempDir, Hosted) {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::initialize(&temp.path().join("service")).unwrap();
    for tenant in ["alpha", "beta"] {
        store.add_tenant(tenant, DataOrigin::Synthetic, 3).unwrap();
    }
    for (name, role, tenant) in [
        ("admin", Role::Admin, "alpha"),
        ("editor", Role::Editor, "beta"),
        ("viewer", Role::Viewer, "alpha"),
    ] {
        store.add_user(name, PASSWORD).unwrap();
        store.set_member(name, tenant, Some(role)).unwrap();
    }
    store
        .set_member("admin", "beta", Some(Role::Admin))
        .unwrap();
    let mut config =
        HostedConfig::parse("https://crm.example.test", "127.0.0.1:4317", false).unwrap();
    let mut bindings = proof::configuration();
    bindings["users"] = json!([
        {"email":"alice@example.invalid","username":"admin"},{"email":"bob@example.invalid","username":"editor"},
        {"email":"carol@example.invalid","username":"viewer"},{"email":"uninvited@example.invalid","username":"uninvited"}]);
    config.access = Some(proof::verifier(&bindings, vec![Ok(proof::fixtures().0.jwks.clone())]).0);
    (temp, Hosted::new(store, config))
}
fn jwt(email: &str) -> String {
    let mut claims = proof::claims();
    claims["email"] = email.into();
    claims["sub"] = format!("synthetic-{email}").into();
    proof::sign(&claims, &proof::fixtures().0)
}
fn request(
    path: &str,
    method: &str,
    jwt: Option<&str>,
    session: Option<&Session>,
    value: &Value,
) -> Input {
    let mut headers = vec![
        ("Host".into(), "crm.example.test".into()),
        ("Origin".into(), "https://crm.example.test".into()),
        ("X-Meibo-Local".into(), "1".into()),
        ("Content-Type".into(), "application/json".into()),
    ];
    if let Some(jwt) = jwt {
        headers.push(("Cf-Access-Jwt-Assertion".into(), jwt.into()));
    }
    if let Some(session) = session {
        headers.push(("Cookie".into(), session.cookie.clone()));
        headers.push(("X-Meibo-Context".into(), session.scope.clone()));
    }
    Input {
        method: method.into(),
        target: path.into(),
        headers,
        body: if method == "POST" {
            serde_json::to_vec(value).unwrap()
        } else {
            vec![]
        },
    }
}
fn login(app: &mut Hosted, email: &str, tenant: &str) -> Session {
    let response = app.dispatch(
        &request(
            "/auth/login",
            "POST",
            Some(&jwt(email)),
            None,
            &json!({"tenant":tenant}),
        ),
        proof::AT,
    );
    assert_eq!(response.reply.status, 200);
    let data: Value = serde_json::from_slice(&response.reply.body).unwrap();
    let cookie = response
        .headers
        .iter()
        .find(|(name, _)| *name == "Set-Cookie")
        .unwrap()
        .1
        .clone();
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("Secure")
            && cookie.contains("SameSite=Strict")
    );
    Session {
        cookie: cookie.split(';').next().unwrap().into(),
        scope: data["storage_scope"].as_str().unwrap().into(),
    }
}

#[test]
fn access_http_rejects_missing_or_duplicate_proofs_before_the_storage_lock() {
    let (_temp, mut app) = fixture();
    let _guard = app.store.lock().unwrap();
    for path in ["/", "/login", "/auth/session", "/api/companies"] {
        assert_eq!(
            app.dispatch(&request(path, "GET", None, None, &Value::Null), proof::AT)
                .reply
                .status,
            401
        );
    }
    let mut email_only = request("/api/companies", "GET", None, None, &Value::Null);
    email_only.headers.push((
        "Cf-Access-Authenticated-User-Email".into(),
        "alice@example.invalid".into(),
    ));
    assert_eq!(app.dispatch(&email_only, proof::AT).reply.status, 401);
    let token = jwt("alice@example.invalid");
    let mut input = request("/api/companies", "GET", Some(&token), None, &Value::Null);
    input
        .headers
        .push(("cf-access-jwt-assertion".into(), token));
    assert_eq!(app.dispatch(&input, proof::AT).reply.status, 401);
}

#[test]
fn access_login_reuses_memberships_without_provisioning_users_or_password_fallback() {
    let (_temp, mut app) = fixture();
    let native = request(
        "/auth/login",
        "POST",
        None,
        None,
        &json!({"tenant":"alpha","username":"admin","password":PASSWORD}),
    );
    assert_eq!(app.dispatch(&native, proof::AT).reply.status, 401);
    for (email, tenant) in [
        ("bob@example.invalid", "alpha"),
        ("uninvited@example.invalid", "alpha"),
    ] {
        let result = app.dispatch(
            &request(
                "/auth/login",
                "POST",
                Some(&jwt(email)),
                None,
                &json!({"tenant":tenant}),
            ),
            proof::AT,
        );
        assert_eq!(result.reply.status, 403);
    }
    let result = app.dispatch(
        &request(
            "/auth/login",
            "POST",
            Some(&jwt("bob@example.invalid")),
            None,
            &json!({"tenant":"alpha","username":"admin","password":PASSWORD}),
        ),
        proof::AT,
    );
    assert_eq!(result.reply.status, 400);
    let count: i64 = app
        .store
        .conn
        .query_row("SELECT count(*) FROM users", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn access_identity_cannot_borrow_or_extend_another_users_cookie() {
    let (_temp, mut app) = fixture();
    let alice = login(&mut app, "alice@example.invalid", "alpha");
    let bob = login(&mut app, "bob@example.invalid", "beta");
    assert_ne!(alice.scope, bob.scope);
    let result = app.dispatch(
        &request(
            "/api/companies",
            "GET",
            Some(&jwt("bob@example.invalid")),
            Some(&alice),
            &Value::Null,
        ),
        proof::AT + 60,
    );
    assert_eq!(result.reply.status, 401);
    let last: i64 = app
        .store
        .conn
        .query_row(
            "SELECT max(last_seen) FROM sessions WHERE tenant='alpha'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(last, proof::AT);
    assert_eq!(
        app.dispatch(
            &request(
                "/api/companies",
                "GET",
                Some(&jwt("alice@example.invalid")),
                Some(&alice),
                &Value::Null
            ),
            proof::AT + 61
        )
        .reply
        .status,
        200
    );
}

#[test]
fn access_sessions_preserve_three_roles_and_existing_context_and_origin_checks() {
    let (_temp, mut app) = fixture();
    for (email, tenant, expected) in [
        ("alice@example.invalid", "alpha", 200),
        ("bob@example.invalid", "beta", 200),
        ("carol@example.invalid", "alpha", 403),
    ] {
        let session = login(&mut app, email, tenant);
        let token = jwt(email);
        let input = request(
            "/api/companies/1/activities",
            "POST",
            Some(&token),
            Some(&session),
            &json!({"note":"synthetic access note"}),
        );
        assert_eq!(app.dispatch(&input, proof::AT + 1).reply.status, expected);
        let mut wrong = input;
        wrong.headers.retain(|(name, _)| name != "X-Meibo-Context");
        wrong
            .headers
            .push(("X-Meibo-Context".into(), "wrong-context".into()));
        assert_eq!(app.dispatch(&wrong, proof::AT + 2).reply.status, 409);
        wrong.headers.retain(|(name, _)| name != "Origin");
        wrong
            .headers
            .push(("Origin".into(), "https://foreign.example".into()));
        assert_eq!(app.dispatch(&wrong, proof::AT + 2).reply.status, 403);
    }
}

#[test]
fn access_logout_revoke_and_disable_keep_existing_session_invalidation() {
    let (_temp, mut app) = fixture();
    let alice = login(&mut app, "alice@example.invalid", "alpha");
    let token = jwt("alice@example.invalid");
    let logout = app.dispatch(
        &request(
            "/auth/logout",
            "POST",
            Some(&token),
            Some(&alice),
            &json!({}),
        ),
        proof::AT + 1,
    );
    assert_eq!(logout.reply.status, 200);
    assert!(
        !logout
            .headers
            .iter()
            .any(|(name, value)| *name == "Location" && value.contains("/cdn-cgi/access/logout"))
    );
    assert_eq!(
        app.dispatch(
            &request(
                "/auth/session",
                "GET",
                Some(&token),
                Some(&alice),
                &Value::Null
            ),
            proof::AT + 2
        )
        .reply
        .status,
        401
    );
    let alice = login(&mut app, "alice@example.invalid", "alpha");
    app.store
        .set_member("admin", "alpha", Some(Role::Viewer))
        .unwrap();
    assert_eq!(
        app.dispatch(
            &request(
                "/auth/session",
                "GET",
                Some(&token),
                Some(&alice),
                &Value::Null
            ),
            proof::AT + 2
        )
        .reply
        .status,
        401
    );
    app.store
        .conn
        .execute("UPDATE users SET disabled=1 WHERE username='admin'", [])
        .unwrap();
    assert_eq!(
        app.dispatch(
            &request(
                "/auth/login",
                "POST",
                Some(&token),
                None,
                &json!({"tenant":"alpha"})
            ),
            proof::AT + 3
        )
        .reply
        .status,
        403
    );
}

#[test]
fn access_login_screen_has_no_password_fields_and_default_password_mode_still_works() {
    let (_temp, mut app) = fixture();
    let token = jwt("alice@example.invalid");
    let page = app.dispatch(
        &request("/login", "GET", Some(&token), None, &Value::Null),
        proof::AT,
    );
    let html = String::from_utf8(page.reply.body).unwrap();
    assert!(html.contains("data-cloudflare-access") && html.contains("Cloudflareでログイン"));
    assert!(!html.contains("id=\"login-password\"") && !html.contains("id=\"login-username\""));
    assert!(html.contains("rel=\"canonical\"") && html.contains("public-share-line"));
    assert_eq!(
        app.dispatch(
            &request("/health", "GET", None, None, &Value::Null),
            proof::AT
        )
        .reply
        .status,
        200
    );
    app.config.access = None;
    let result = app.dispatch(
        &request(
            "/auth/login",
            "POST",
            None,
            None,
            &json!({"tenant":"alpha","username":"admin","password":PASSWORD}),
        ),
        proof::AT,
    );
    assert_eq!(result.reply.status, 200);
}

/// Starts the real HTTP implementation with a synthetic authority for the
/// operator-run browser suite. This helper is absent from the production binary.
#[test]
#[ignore = "operator-run HTTPS/Chromium fixture; exercised by cloudflare-access-browser.mjs"]
fn access_browser_fixture() {
    let origin = std::env::var("MEIBO_ACCESS_QA_ORIGIN").expect("explicit local QA origin");
    assert!(origin.starts_with("https://localhost:"));
    let token_path = std::path::PathBuf::from(
        std::env::var("MEIBO_ACCESS_QA_TOKENS").expect("private token destination"),
    );
    let (_temp, mut app) = fixture();
    let mut config = HostedConfig::parse(&origin, "127.0.0.1:0", false).unwrap();
    let mut bindings = proof::configuration();
    bindings["users"] = json!([{"email":"alice@example.invalid","username":"admin"},{"email":"bob@example.invalid","username":"editor"},{"email":"carol@example.invalid","username":"viewer"}]);
    config.access =
        Some(proof::verifier(&bindings, vec![Ok(proof::fixtures().0.jwks.clone()); 16]).0);
    let mode = std::env::var("MEIBO_ACCESS_QA_MODE").unwrap_or_else(|_| "access".into());
    assert!(matches!(mode.as_str(), "access" | "password"));
    if mode == "password" {
        config.access = None;
    }
    app.config = config;
    let at = crate::hosted::now().unwrap();
    let mut tokens = serde_json::Map::new();
    for (role, email) in [
        ("admin", "alice@example.invalid"),
        ("editor", "bob@example.invalid"),
        ("viewer", "carol@example.invalid"),
    ] {
        let mut claims = proof::claims();
        claims["email"] = email.into();
        claims["sub"] = format!("synthetic-{role}").into();
        claims["iat"] = (at - 1).into();
        claims["exp"] = (at + 3600).into();
        tokens.insert(
            role.into(),
            proof::sign(&claims, &proof::fixtures().0).into(),
        );
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(token_path)
        .unwrap()
        .write_all(&serde_json::to_vec(&tokens).unwrap())
        .unwrap();
    app.serve().unwrap();
}
