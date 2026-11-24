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

#[test]
fn access_login_html_bytes_are_unchanged_after_demo_markers() {
    let (_temp, mut app) = fixture();
    let token = jwt("alice@example.invalid");
    let page = app.dispatch(
        &request("/login", "GET", Some(&token), None, &Value::Null),
        proof::AT,
    );
    assert_eq!(page.reply.status, 200);
    let html = String::from_utf8(page.reply.body).unwrap();
    let canonical = "https://crm.example.test/login";
    let image = "https://crm.example.test/public-entry.png";
    let encoded_url = "https%3A%2F%2Fcrm.example.test%2Flogin";
    let encoded_text = "meibo%20%C2%B7%20%E5%96%B6%E6%A5%AD%E3%83%AF%E3%83%BC%E3%82%AF%E3%82%B9%E3%83%9A%E3%83%BC%E3%82%B9";
    let x_url =
        format!("https://twitter.com/intent/tweet?url={encoded_url}&amp;text={encoded_text}");
    let line_url = format!(
        "https://social-plugins.line.me/lineit/share?url={encoded_url}&amp;text={encoded_text}"
    );
    let expected = format!(
        r#"<!doctype html>
<html lang="ja">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <meta name="color-scheme" content="light">
  <title>ログイン · meibo</title>
  <meta name="description" content="企業情報と営業活動を、チームのワークスペースで。">
  <link rel="canonical" href="{canonical}">
  <meta property="og:title" content="meibo · 営業ワークスペース">
  <meta property="og:type" content="website">
  <meta property="og:url" content="{canonical}">
  <meta property="og:description" content="企業情報と営業活動を、チームのワークスペースで。">
  <meta property="og:image" content="{image}">
  <meta property="og:image:type" content="image/png">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="meibo · 営業ワークスペース">
  <meta name="twitter:card" content="summary_large_image">
  <script src="/public-entry.js" defer></script>
  <link rel="stylesheet" href="/login.css">
  <script src="/login.js" defer></script>
</head>
<body class="login-page">
  <main class="login-card">
    <a class="login-brand" href="/login" aria-label="meibo ログイン">m<span>meibo</span></a>
    <h1>営業ワークスペースへ</h1>
    <p class="login-intro">Cloudflareで確認したアカウントを使います。招待された組織IDを入力してください。</p>
    <form id="login-form" data-cloudflare-access>
      <fieldset id="login-fields">
        <div class="login-field"><label for="login-tenant">組織ID</label><input id="login-tenant" name="tenant" required maxlength="48" pattern="[a-z0-9][a-z0-9-]*" autocomplete="off" autocapitalize="none" spellcheck="false" aria-describedby="tenant-hint"><p id="tenant-hint">招待時に案内された組織IDを入力してください。</p></div>
        
        <div id="login-error" class="login-error" role="alert" hidden></div>
        <button id="login-submit" type="submit">Cloudflareでログイン</button>
      </fieldset>
    </form>
    <p class="login-help">アカウントや組織IDが分からない場合は、招待した管理者へお問い合わせください。</p>
    
    <footer id="public-entry-share" class="public-share" aria-labelledby="public-share-heading">
      <h2 id="public-share-heading">ログインURLを共有</h2>
      <p class="public-share-help">ログイン画面のURLを共有します。ご利用には管理者からの案内が必要です。</p>
      <div class="public-share-actions">
        <a id="public-share-x" href="{x_url}" target="_blank" rel="noopener noreferrer" aria-label="Xで共有（新しいタブ）">X</a>
        <a id="public-share-line" href="{line_url}" target="_blank" rel="noopener noreferrer" aria-label="LINEで共有（新しいタブ）">LINE</a>
        <button id="public-share-copy" type="button" disabled>リンクをコピー</button>
        <button id="public-share-native" type="button" hidden>端末で共有</button>
      </div>
      <p id="public-share-status" class="public-share-status" role="status" aria-live="polite" aria-atomic="true"></p>
      <div id="public-share-fallback" class="public-share-fallback" hidden>
        <label for="public-share-url">ログインURL（選択してコピー）</label>
        <input id="public-share-url" type="url" readonly value="{canonical}" autocomplete="off" spellcheck="false">
      </div>
    </footer>
    
  </main>
</body>
</html>
"#
    );
    assert_eq!(html, expected);
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

mod leads;
