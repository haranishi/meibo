use std::time::Duration;

use super::*;
use crate::hosted::demo::{self, DemoConfig};
use serde_json::{Value, json};

const DEMO_HOST: &str = "demo.example.test";
const DEMO_ORIGIN: &str = "https://demo.example.test";

fn demo_config(contact: Option<&str>) -> HostedConfig {
    let mut cfg = HostedConfig::parse(DEMO_ORIGIN, "127.0.0.1:4317", false).unwrap();
    cfg.demo = Some(DemoConfig::new(contact.map(str::to_owned)).unwrap());
    cfg
}

fn demo_fixture(contact: Option<&str>) -> (tempfile::TempDir, Hosted) {
    let temp = tempfile::tempdir().unwrap();
    let store = demo::bootstrap(&temp.path().join("demo"), AT).unwrap();
    (temp, Hosted::new(store, demo_config(contact)))
}

fn demo_request(
    path: &str,
    method: &str,
    cookie: Option<&str>,
    scope: Option<&str>,
    body: Vec<u8>,
) -> crate::server::Input {
    let mut headers = vec![
        ("Host".into(), DEMO_HOST.into()),
        ("Origin".into(), DEMO_ORIGIN.into()),
        ("X-Meibo-Local".into(), "1".into()),
        ("Content-Type".into(), "application/json".into()),
    ];
    if let Some(cookie) = cookie {
        headers.push(("Cookie".into(), cookie.into()));
    }
    if let Some(scope) = scope {
        headers.push(("X-Meibo-Context".into(), scope.into()));
    }
    crate::server::Input {
        method: method.into(),
        target: path.into(),
        headers,
        body,
    }
}

fn enter(app: &mut Hosted, at: i64) -> (String, String, Value) {
    let reply = app.dispatch(
        &demo_request("/demo/enter", "POST", None, None, b"{}".to_vec()),
        at,
    );
    assert_eq!(reply.reply.status, 200);
    let set_cookie = reply.header("Set-Cookie").expect("Set-Cookie").to_owned();
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    let body: Value = serde_json::from_slice(&reply.reply.body).unwrap();
    let scope = body["storage_scope"].as_str().unwrap().to_owned();
    (cookie, scope, body)
}

#[test]
fn demo_enter_issues_editor_session_and_rejects_password_login() {
    let (_temp, mut app) = demo_fixture(None);
    let reply = app.dispatch(
        &demo_request("/demo/enter", "POST", None, None, b"{}".to_vec()),
        AT,
    );
    assert_eq!(reply.reply.status, 200);
    let set_cookie = reply.header("Set-Cookie").expect("Set-Cookie header");
    for attr in [
        "__Host-meibo-session=",
        "Path=/",
        "HttpOnly",
        "SameSite=Strict",
        "Max-Age=28800",
        "Secure",
    ] {
        assert!(
            set_cookie.contains(attr),
            "missing cookie attr {attr}: {set_cookie}"
        );
    }
    let body: Value = serde_json::from_slice(&reply.reply.body).unwrap();
    assert_eq!(body["username"], "demo");
    assert_eq!(body["tenant"], "demo");
    assert_eq!(body["role"], "editor");
    let scope = body["storage_scope"].as_str().unwrap();
    assert_eq!(scope.len(), 64);

    let cookie = set_cookie.split(';').next().unwrap();
    let session = app.dispatch(
        &demo_request("/auth/session", "GET", Some(cookie), None, Vec::new()),
        AT + 1,
    );
    assert_eq!(session.reply.status, 200);
    let session_body: Value = serde_json::from_slice(&session.reply.body).unwrap();
    assert_eq!(session_body["username"], "demo");
    assert_eq!(session_body["tenant"], "demo");
    assert_eq!(session_body["role"], "editor");
    assert_eq!(session_body["storage_scope"], scope);

    // デモ中は形式が正しい本文でも POST /auth/login を404で拒否し、パスワード照合を行わない。
    let password_login = demo_request(
        "/auth/login",
        "POST",
        None,
        None,
        json!({"tenant":"demo","username":"demo","password":PASSWORD})
            .to_string()
            .into_bytes(),
    );
    for _ in 0..100 {
        assert_eq!(app.dispatch(&password_login, AT + 2).reply.status, 404);
    }
    let login_reply = app.dispatch(&password_login, AT + 2);
    assert_eq!(login_reply.reply.status, 404);
    let login_err: Value = serde_json::from_slice(&login_reply.reply.body).unwrap();
    assert!(login_err["error"].as_str().is_some_and(|m| !m.is_empty()));
    let attempts: i64 = app
        .store
        .conn
        .query_row("SELECT count(*) FROM login_attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(attempts, 0);
}

#[test]
fn demo_enter_rate_limits_per_minute_without_blocking_active_sessions() {
    let (_temp, mut app) = demo_fixture(None);
    let first_reply = app.dispatch(
        &demo_request("/demo/enter", "POST", None, None, b"{}".to_vec()),
        AT,
    );
    assert_eq!(first_reply.reply.status, 200);
    let first_set_cookie = first_reply
        .header("Set-Cookie")
        .expect("Set-Cookie")
        .to_owned();
    let first_cookie = first_set_cookie.split(';').next().unwrap().to_owned();
    let first_scope = json_body(&first_reply)["storage_scope"]
        .as_str()
        .unwrap()
        .to_owned();

    // 同じ1分窓で2回目〜300回目まで許可され、すべて同じSet-Cookieが返る。
    for i in 1_i64..300 {
        let reply = app.dispatch(
            &demo_request("/demo/enter", "POST", None, None, b"{}".to_vec()),
            AT + (i % 59),
        );
        assert_eq!(reply.reply.status, 200, "entry #{i} should succeed");
        assert_eq!(
            reply.header("Set-Cookie"),
            Some(first_set_cookie.as_str()),
            "entry #{i} should reuse shared Set-Cookie"
        );
    }
    let stored: i64 = app
        .store
        .conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 1);

    // 同じ分の301回目は429（demo_busy）になる。
    let throttled = app.dispatch(
        &demo_request("/demo/enter", "POST", None, None, b"{}".to_vec()),
        AT + 59,
    );
    assert_eq!(throttled.reply.status, 429);
    let throttled_body: Value = serde_json::from_slice(&throttled.reply.body).unwrap();
    assert_eq!(
        throttled_body,
        json!({
            "error": "混み合っています。少し待ってからもう一度お試しください。",
            "code": "demo_busy"
        })
    );

    // 429のあとも、先に入室したCookieの操作は200で継続できる。
    let session = app.dispatch(
        &demo_request(
            "/auth/session",
            "GET",
            Some(&first_cookie),
            None,
            Vec::new(),
        ),
        AT + 59,
    );
    assert_eq!(session.reply.status, 200);
    let summary = app.dispatch(
        &demo_request(
            "/api/summary",
            "GET",
            Some(&first_cookie),
            Some(&first_scope),
            Vec::new(),
        ),
        AT + 59,
    );
    assert_eq!(summary.reply.status, 200);

    // 次の分に入ると入室が再び成功し、同じ共有sessionを返す。
    let next_minute = app.dispatch(
        &demo_request("/demo/enter", "POST", None, None, b"{}".to_vec()),
        AT + 60,
    );
    assert_eq!(next_minute.reply.status, 200);
    assert_eq!(
        next_minute.header("Set-Cookie"),
        Some(first_set_cookie.as_str())
    );
    let stored_after: i64 = app
        .store
        .conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored_after, 1);
}

#[test]
fn demo_shares_single_session_without_eviction_and_normal_login_still_keeps_20() {
    // デモ: 複数分にまたがって350回入室してもsessions表は1行のままで、最初のCookieも有効のまま。
    let (_temp, mut app) = demo_fixture(None);
    let (first_cookie, first_scope, _) = enter(&mut app, AT);
    for i in 1_i64..350 {
        let ts = AT + (i / 300) * 60 + (i % 60);
        let (cookie, _, _) = enter(&mut app, ts);
        assert_eq!(cookie, first_cookie, "entry #{i} should share token");
    }
    let last_ts = AT + 60 + 50;
    let stored: i64 = app
        .store
        .conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 1);
    assert_eq!(
        app.dispatch(
            &demo_request(
                "/api/summary",
                "GET",
                Some(&first_cookie),
                Some(&first_scope),
                Vec::new(),
            ),
            last_ts + 1,
        )
        .reply
        .status,
        200
    );

    // 別起動（初期化後）は別の共有トークンになり、旧Cookieは /login?reset=1 へ戻される。
    let (_temp2, mut restarted) = demo_fixture(None);
    let (restarted_cookie, _, _) = enter(&mut restarted, last_ts + 2);
    assert_ne!(restarted_cookie, first_cookie);
    let stale_root = restarted.dispatch(
        &demo_request("/", "GET", Some(&first_cookie), None, Vec::new()),
        last_ts + 3,
    );
    assert_eq!(stale_root.reply.status, 302);
    assert_eq!(stale_root.header("Location"), Some("/login?reset=1"));

    // 通常モード回帰: パスワードログインは従来どおり20件だけ保持する。
    let (_normal_temp, mut store) = fixture();
    let mut tokens = Vec::with_capacity(21);
    for i in 0_i64..21 {
        let LoginOutcome::Success(session) =
            store.login("alpha", "operator", PASSWORD, AT + i).unwrap()
        else {
            panic!("normal login #{i} should succeed");
        };
        tokens.push(session.token);
    }
    let normal_count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(normal_count, 20);
    assert!(store.authenticate(&tokens[0], AT + 30).unwrap().is_none());
    assert!(store.authenticate(&tokens[1], AT + 30).unwrap().is_some());
    assert!(store.authenticate(&tokens[20], AT + 30).unwrap().is_some());
}

#[test]
fn demo_logout_clears_cookie_without_revoking_shared_session_for_other_visitors() {
    let (_temp, mut app) = demo_fixture(None);
    let (leaving_cookie, leaving_scope, _) = enter(&mut app, AT);
    let (staying_cookie, staying_scope, _) = enter(&mut app, AT + 1);
    assert_eq!(leaving_cookie, staying_cookie);
    assert_eq!(leaving_scope, staying_scope);

    // 訪問者Aがログアウトすると、Cookie削除ヘッダーと Clear-Site-Data を返す。
    let logout = app.dispatch(
        &demo_request(
            "/auth/logout",
            "POST",
            Some(&leaving_cookie),
            Some(&leaving_scope),
            b"{}".to_vec(),
        ),
        AT + 2,
    );
    assert_eq!(logout.reply.status, 200);
    let cleared = logout.header("Set-Cookie").expect("Set-Cookie on logout");
    assert!(cleared.starts_with("__Host-meibo-session=;"));
    assert!(cleared.contains("Max-Age=0"));
    assert_eq!(
        logout.header("Clear-Site-Data"),
        Some("\"cache\", \"storage\"")
    );

    // 同じトークンを持つ訪問者Bは引き続き200で操作でき、sessions表も1行のまま。
    assert_eq!(
        app.dispatch(
            &demo_request(
                "/auth/session",
                "GET",
                Some(&staying_cookie),
                None,
                Vec::new(),
            ),
            AT + 3,
        )
        .reply
        .status,
        200
    );
    assert_eq!(
        app.dispatch(
            &demo_request(
                "/api/summary",
                "GET",
                Some(&staying_cookie),
                Some(&staying_scope),
                Vec::new(),
            ),
            AT + 3,
        )
        .reply
        .status,
        200
    );
    let stored: i64 = app
        .store
        .conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 1);
}

#[test]
fn demo_never_writes_audit_rows_while_normal_hosted_continues_auditing() {
    let (_temp, mut app) = demo_fixture(None);
    let audit_count = |store: &Store| -> i64 {
        store
            .conn
            .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(audit_count(&app.store), 0);

    let (cookie, scope, _) = enter(&mut app, AT);
    assert_eq!(
        demo_post_json(
            &mut app,
            &cookie,
            &scope,
            "/api/companies",
            &json!({"name":"合成・監査確認株式会社","prefecture":13}),
        )
        .reply
        .status,
        201
    );
    // 編集者が管理者専用APIや /auth/audit を呼んで403になっても監査行は増えない。
    assert_eq!(
        demo_post_json(
            &mut app,
            &cookie,
            &scope,
            "/api/sales-targets",
            &json!({"month":"2026-11","owner_id":1,"amount_jpy":100}),
        )
        .reply
        .status,
        403
    );
    assert_eq!(
        demo_post_json(
            &mut app,
            &cookie,
            &scope,
            "/api/companies/1/contacts/1/erase",
            &json!({"wipe_notes":false}),
        )
        .reply
        .status,
        403
    );
    assert_eq!(
        app.dispatch(
            &demo_request(
                "/auth/audit",
                "GET",
                Some(&cookie),
                Some(&scope),
                Vec::new()
            ),
            AT + 1,
        )
        .reply
        .status,
        403
    );
    assert_eq!(
        demo_post_json(&mut app, &cookie, &scope, "/auth/logout", &json!({}))
            .reply
            .status,
        200
    );
    assert_eq!(audit_count(&app.store), 0);
}

#[test]
fn normal_hosted_still_writes_audit_rows() {
    let audit_count = |store: &Store| -> i64 {
        store
            .conn
            .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
            .unwrap()
    };
    // 通常モード（デモなし）では従来どおり監査行が増える。
    let (_normal_temp, store) = fixture();
    let mut normal = Hosted::new(store, config());
    let before = audit_count(&normal.store);
    assert!(before > 0);
    let admin = login(&mut normal.store, "alpha");
    let viewer = login(&mut normal.store, "beta");
    assert_eq!(
        normal
            .dispatch(
                &request(
                    "/api/companies",
                    "POST",
                    Some(&admin),
                    &json!({"name":"合成・通常監査株式会社","prefecture":13}),
                ),
                AT + 1,
            )
            .reply
            .status,
        201
    );
    assert_eq!(
        normal
            .dispatch(
                &request("/auth/audit", "GET", Some(&viewer), &json!(null)),
                AT + 1
            )
            .reply
            .status,
        403
    );
    assert_eq!(
        normal
            .dispatch(
                &request("/auth/logout", "POST", Some(&admin), &json!({})),
                AT + 2
            )
            .reply
            .status,
        200
    );
    let after = audit_count(&normal.store);
    assert!(
        after >= before + 5,
        "expected audit rows to grow: before={before} after={after}"
    );
    let operations: Vec<String> = normal
        .store
        .audit_rows(None, 20)
        .unwrap()
        .into_iter()
        .map(|row| row["operation"].as_str().unwrap().to_owned())
        .collect();
    for expected in ["login", "crm.write", "authorization", "logout"] {
        assert!(
            operations.iter().any(|op| op == expected),
            "missing normal audit operation {expected}: {operations:?}"
        );
    }
}

#[test]
fn demo_enter_enforces_http_boundary_body_shape_and_256_byte_limit() {
    let (_temp, mut app) = demo_fixture(None);

    // Host不一致 -> 403
    let mut bad_host = demo_request("/demo/enter", "POST", None, None, b"{}".to_vec());
    bad_host
        .headers
        .iter_mut()
        .find(|(k, _)| k == "Host")
        .unwrap()
        .1 = "evil.example.test".into();
    assert_eq!(app.dispatch(&bad_host, AT).reply.status, 403);

    // Origin欠落 -> 403
    let mut no_origin = demo_request("/demo/enter", "POST", None, None, b"{}".to_vec());
    no_origin.headers.retain(|(k, _)| k != "Origin");
    assert_eq!(app.dispatch(&no_origin, AT).reply.status, 403);

    // X-Meibo-Local欠落 -> 403
    let mut no_local = demo_request("/demo/enter", "POST", None, None, b"{}".to_vec());
    no_local.headers.retain(|(k, _)| k != "X-Meibo-Local");
    assert_eq!(app.dispatch(&no_local, AT).reply.status, 403);

    // Content-Type違い -> 415
    let mut bad_ct = demo_request("/demo/enter", "POST", None, None, b"{}".to_vec());
    bad_ct
        .headers
        .iter_mut()
        .find(|(k, _)| k == "Content-Type")
        .unwrap()
        .1 = "text/plain".into();
    assert_eq!(app.dispatch(&bad_ct, AT).reply.status, 415);

    // 本文がJSONでない・{}でない -> 400
    for body in [
        b"".as_slice(),
        b"not-json".as_slice(),
        b"null".as_slice(),
        b"[]".as_slice(),
        b"{\"tenant\":\"demo\"}".as_slice(),
    ] {
        let reply = app.dispatch(
            &demo_request("/demo/enter", "POST", None, None, body.to_vec()),
            AT,
        );
        assert_eq!(
            reply.reply.status,
            400,
            "unexpected status for body {:?}",
            String::from_utf8_lossy(body)
        );
    }

    // 257バイト以上 -> 413、ちょうど256バイトの{} -> 200
    let mut too_large = vec![b' '; 255];
    too_large.extend_from_slice(b"{}");
    assert_eq!(too_large.len(), 257);
    assert_eq!(
        app.dispatch(
            &demo_request("/demo/enter", "POST", None, None, too_large),
            AT,
        )
        .reply
        .status,
        413
    );

    let mut exact_limit = vec![b' '; 254];
    exact_limit.extend_from_slice(b"{}");
    assert_eq!(exact_limit.len(), 256);
    assert_eq!(
        app.dispatch(
            &demo_request("/demo/enter", "POST", None, None, exact_limit),
            AT,
        )
        .reply
        .status,
        200
    );
}

#[test]
fn non_demo_rejects_demo_enter_with_404_and_preserves_login_html_bytes() {
    let (_temp, store) = fixture();
    let mut app = Hosted::new(store, config());
    // 非デモでは POST /demo/enter は404、POST /auth/login は従来どおり200。
    assert_eq!(
        app.dispatch(&request("/demo/enter", "POST", None, &json!({})), AT)
            .reply
            .status,
        404
    );
    let session = login(&mut app.store, "alpha");
    assert_eq!(
        app.dispatch(
            &request("/demo/enter", "POST", Some(&session), &json!({})),
            AT
        )
        .reply
        .status,
        404
    );
    assert_eq!(
        app.dispatch(
            &request(
                "/auth/login",
                "POST",
                None,
                &json!({"tenant":"alpha","username":"operator","password":PASSWORD})
            ),
            AT
        )
        .reply
        .status,
        200
    );

    // 通常モードと開発モードの GET /login 出力が、デモ用マーカー追加前と1バイトも変わらない。
    let login_page = app.dispatch(&request("/login", "GET", None, &json!(null)), AT);
    assert_eq!(login_page.reply.status, 200);
    let html = String::from_utf8(login_page.reply.body).unwrap();
    assert_eq!(html, expected_https_login_html("https://crm.example.test"));

    let dev_config = HostedConfig::parse("http://127.0.0.1:4317", "127.0.0.1:4317", true).unwrap();
    let dev_html = String::from_utf8(crate::hosted::public_entry::login(&dev_config).body).unwrap();
    assert_eq!(dev_html, expected_dev_login_html());
}

fn expected_dev_login_html() -> &'static str {
    concat!(
        "<!doctype html>\n",
        "<html lang=\"ja\">\n",
        "<head>\n",
        "  <meta charset=\"utf-8\">\n",
        "  <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n",
        "  <meta name=\"color-scheme\" content=\"light\">\n",
        "  <title>ログイン · meibo</title>\n",
        "  \n",
        "  <link rel=\"stylesheet\" href=\"/login.css\">\n",
        "  <script src=\"/login.js\" defer></script>\n",
        "</head>\n",
        "<body class=\"login-page\">\n",
        "  <main class=\"login-card\">\n",
        "    <a class=\"login-brand\" href=\"/login\" aria-label=\"meibo ログイン\">m<span>meibo</span></a>\n",
        "    <h1>営業ワークスペースへ</h1>\n",
        "    <p class=\"login-intro\">招待された組織のアカウントでログインしてください。</p>\n",
        "    <form id=\"login-form\">\n",
        "      <fieldset id=\"login-fields\">\n",
        "        <div class=\"login-field\"><label for=\"login-tenant\">組織ID</label><input id=\"login-tenant\" name=\"tenant\" required maxlength=\"48\" pattern=\"[a-z0-9][a-z0-9-]*\" autocomplete=\"off\" autocapitalize=\"none\" spellcheck=\"false\" aria-describedby=\"tenant-hint\"><p id=\"tenant-hint\">招待時に案内された組織IDを入力してください。</p></div>\n",
        "        \n",
        "        <div class=\"login-field\"><label for=\"login-username\">利用者名</label><input id=\"login-username\" name=\"username\" required maxlength=\"128\" autocomplete=\"username\" autocapitalize=\"none\" spellcheck=\"false\"></div>\n",
        "        <div class=\"login-field\"><label for=\"login-password\">パスワード</label><input id=\"login-password\" name=\"password\" type=\"password\" required maxlength=\"1024\" autocomplete=\"current-password\"></div>\n",
        "        \n",
        "        <div id=\"login-error\" class=\"login-error\" role=\"alert\" hidden></div>\n",
        "        <button id=\"login-submit\" type=\"submit\">ログイン</button>\n",
        "      </fieldset>\n",
        "    </form>\n",
        "    <p class=\"login-help\">アカウントや組織IDが分からない場合は、招待した管理者へお問い合わせください。</p>\n",
        "    \n",
        "  </main>\n",
        "</body>\n",
        "</html>\n",
    )
}

fn expected_https_login_html(origin: &str) -> String {
    let canonical = format!("{origin}/login");
    let image = format!("{origin}/public-entry.png");
    let encoded_url = canonical.replace(':', "%3A").replace('/', "%2F");
    let encoded_text = "meibo%20%C2%B7%20%E5%96%B6%E6%A5%AD%E3%83%AF%E3%83%BC%E3%82%AF%E3%82%B9%E3%83%9A%E3%83%BC%E3%82%B9";
    let x_url =
        format!("https://twitter.com/intent/tweet?url={encoded_url}&amp;text={encoded_text}");
    let line_url = format!(
        "https://social-plugins.line.me/lineit/share?url={encoded_url}&amp;text={encoded_text}"
    );
    format!(
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
    <p class="login-intro">招待された組織のアカウントでログインしてください。</p>
    <form id="login-form">
      <fieldset id="login-fields">
        <div class="login-field"><label for="login-tenant">組織ID</label><input id="login-tenant" name="tenant" required maxlength="48" pattern="[a-z0-9][a-z0-9-]*" autocomplete="off" autocapitalize="none" spellcheck="false" aria-describedby="tenant-hint"><p id="tenant-hint">招待時に案内された組織IDを入力してください。</p></div>
        
        <div class="login-field"><label for="login-username">利用者名</label><input id="login-username" name="username" required maxlength="128" autocomplete="username" autocapitalize="none" spellcheck="false"></div>
        <div class="login-field"><label for="login-password">パスワード</label><input id="login-password" name="password" type="password" required maxlength="1024" autocomplete="current-password"></div>
        
        <div id="login-error" class="login-error" role="alert" hidden></div>
        <button id="login-submit" type="submit">ログイン</button>
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
    )
}

#[test]
fn demo_login_entry_renders_demo_copy_ogp_share_notes_and_optional_contact() {
    let (_temp, mut app) = demo_fixture(Some("https://example.invalid/issues?a=1&b=2"));
    let reply = app.dispatch(&demo_request("/login", "GET", None, None, Vec::new()), AT);
    assert_eq!(reply.reply.status, 200);
    let html = String::from_utf8(reply.reply.body).unwrap();

    for expected in [
        "<title>meibo · 公開デモ</title>",
        "<meta name=\"description\" content=\"合成データの営業CRMを、そのまま触って試せる公開デモです。\">",
        "<link rel=\"canonical\" href=\"https://demo.example.test/login\">",
        "<meta property=\"og:title\" content=\"meibo · 公開デモ\">",
        "<meta property=\"og:url\" content=\"https://demo.example.test/login\">",
        "<meta property=\"og:description\" content=\"合成データの営業CRMを、そのまま触って試せる公開デモです。\">",
        "<meta property=\"og:image\" content=\"https://demo.example.test/public-entry.png\">",
        "<meta name=\"twitter:card\" content=\"summary_large_image\">",
        "<script src=\"/public-entry.js\" defer></script>",
        "id=\"demo-enter\"",
        ">デモを開く</button>",
        "<h1>営業ワークスペースを試す</h1>",
        "ボタンを押すと、合成データだけのデモに入れます。",
        "データはすべて合成です。実在の企業や個人とは関係ありません。",
        "全員が同じデモを共有します。他の人の操作で表示が変わることがあります。",
        "15分操作がないと初期化されます。",
        "個人情報を入力しないでください。",
        "<a href=\"https://example.invalid/issues?a=1&amp;b=2\" target=\"_blank\" rel=\"noopener noreferrer\">不具合や質問はこちら</a>",
        "id=\"public-entry-share\"",
        "デモのURLを共有",
        "このデモのURLを共有できます。",
        "id=\"public-share-x\"",
        "id=\"public-share-line\"",
        "id=\"public-share-copy\"",
        "id=\"public-share-native\"",
        "id=\"demo-reset-notice\"",
    ] {
        assert!(
            html.contains(expected),
            "missing demo login content: {expected}"
        );
    }
    for forbidden in [
        "id=\"login-form\"",
        "id=\"login-tenant\"",
        "id=\"login-username\"",
        "id=\"login-password\"",
        "id=\"login-submit\"",
        "<!-- ",
        "{{",
    ] {
        assert!(
            !html.contains(forbidden),
            "unexpected content in demo login: {forbidden}"
        );
    }

    // 連絡先なし & idle_timeout変更時の分数表示を確認する。
    let mut cfg = demo_config(None);
    cfg.demo.as_mut().unwrap().idle_timeout = Duration::from_secs(600);
    let no_contact_html = String::from_utf8(crate::hosted::public_entry::login(&cfg).body).unwrap();
    assert!(no_contact_html.contains("10分操作がないと初期化されます。"));
    assert!(!no_contact_html.contains("不具合や質問はこちら"));
    assert!(!no_contact_html.contains("demo-contact"));
}

#[test]
fn unauthenticated_root_redirects_to_reset_notice_only_for_stale_demo_cookie() {
    let (_temp, mut app) = demo_fixture(None);

    // 旧session Cookieあり -> /login?reset=1
    let stale = app.dispatch(
        &demo_request(
            "/",
            "GET",
            Some("__Host-meibo-session=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            None,
            Vec::new(),
        ),
        AT,
    );
    assert_eq!(stale.reply.status, 302);
    assert_eq!(stale.header("Location"), Some("/login?reset=1"));

    // Cookieなし -> /login
    let no_cookie = app.dispatch(&demo_request("/", "GET", None, None, Vec::new()), AT);
    assert_eq!(no_cookie.reply.status, 302);
    assert_eq!(no_cookie.header("Location"), Some("/login"));

    // 無関係なCookieのみ -> /login
    let other_cookie = app.dispatch(
        &demo_request("/", "GET", Some("other=value"), None, Vec::new()),
        AT,
    );
    assert_eq!(other_cookie.reply.status, 302);
    assert_eq!(other_cookie.header("Location"), Some("/login"));

    // 通常モードでは旧Cookieがあっても /login のまま。
    let (_normal_temp, store) = fixture();
    let mut normal = Hosted::new(store, config());
    let mut normal_req = request("/", "GET", None, &json!(null));
    normal_req.headers.push((
        "Cookie".into(),
        "__Host-meibo-session=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .into(),
    ));
    let normal_redirect = normal.dispatch(&normal_req, AT);
    assert_eq!(normal_redirect.reply.status, 302);
    assert_eq!(normal_redirect.header("Location"), Some("/login"));
}

#[test]
fn authenticated_demo_workspace_adds_banner_and_serves_demo_css_only_in_demo() {
    let (_temp, mut app) = demo_fixture(Some("https://example.invalid/issues"));
    let (cookie, _, _) = enter(&mut app, AT);

    // 認証後の GET / に注意書き帯・連絡先リンク・/demo.css・既存の合成デモバッジが含まれる。
    let page = app.dispatch(
        &demo_request("/", "GET", Some(&cookie), None, Vec::new()),
        AT + 1,
    );
    assert_eq!(page.reply.status, 200);
    let html = String::from_utf8(page.reply.body).unwrap();
    let body_pos = html.find("<body>").expect("<body>");
    let banner_pos = html
        .find("<div class=\"demo-banner\" role=\"note\">")
        .expect("demo banner");
    let skip_pos = html.find("class=\"skip-link\"").expect("skip link");
    assert!(body_pos < banner_pos && banner_pos < skip_pos);
    for expected in [
        "<link rel=\"stylesheet\" href=\"/demo.css\">",
        "role=\"note\"",
        "公開デモ · データはすべて合成です · 15分操作がないと初期化されます · 個人情報を入力しないでください",
        "<a href=\"https://example.invalid/issues\" target=\"_blank\" rel=\"noopener noreferrer\">不具合を知らせる</a>",
        "class=\"demo-badge\"",
        "合成デモ",
    ] {
        assert!(
            html.contains(expected),
            "missing workspace demo item: {expected}"
        );
    }
    assert!(!html.contains("style="));

    // 連絡先なしのデモでは「不具合を知らせる」リンクを出さない。
    let (_temp2, mut app_no_contact) = demo_fixture(None);
    let (cookie2, _, _) = enter(&mut app_no_contact, AT);
    let page2 = app_no_contact.dispatch(
        &demo_request("/", "GET", Some(&cookie2), None, Vec::new()),
        AT + 1,
    );
    let html2 = String::from_utf8(page2.reply.body).unwrap();
    assert!(html2.contains("role=\"note\""));
    assert!(!html2.contains("不具合を知らせる"));

    // /demo.css はデモかつ認証後のときだけ200。未認証は401。
    assert_eq!(
        app.dispatch(
            &demo_request("/demo.css", "GET", None, None, Vec::new()),
            AT
        )
        .reply
        .status,
        401
    );
    let css = app.dispatch(
        &demo_request("/demo.css", "GET", Some(&cookie), None, Vec::new()),
        AT + 1,
    );
    assert_eq!(css.reply.status, 200);
    assert_eq!(css.reply.mime, "text/css; charset=utf-8");
    let css_text = String::from_utf8(css.reply.body).unwrap();
    for selector in [
        ".demo-banner",
        "#import-csv",
        "#sales-connections",
        ".lead-inbox-open",
        "#activity-sync-root",
        "display: none",
    ] {
        assert!(css_text.contains(selector), "demo.css missing {selector}");
    }

    // 通常モードでは GET / に帯や demo.css が無く、/demo.css は未知の経路と同じ404。
    let (_normal_temp, store) = fixture();
    let mut normal = Hosted::new(store, config());
    let alpha = login(&mut normal.store, "alpha");
    let normal_page = normal.dispatch(&request("/", "GET", Some(&alpha), &json!(null)), AT);
    assert_eq!(normal_page.reply.status, 200);
    let normal_html = String::from_utf8(normal_page.reply.body).unwrap();
    for forbidden in [
        "demo-banner",
        "role=\"note\"",
        "/demo.css",
        "不具合を知らせる",
    ] {
        assert!(!normal_html.contains(forbidden));
    }
    let unknown = normal.dispatch(
        &request("/unknown-asset.css", "GET", Some(&alpha), &json!(null)),
        AT,
    );
    let normal_demo_css =
        normal.dispatch(&request("/demo.css", "GET", Some(&alpha), &json!(null)), AT);
    assert_eq!(normal_demo_css.reply.status, unknown.reply.status);
    assert_eq!(normal_demo_css.reply.body, unknown.reply.body);
    assert_eq!(
        normal
            .dispatch(&request("/demo.css", "GET", None, &json!(null)), AT)
            .reply
            .status,
        401
    );
}

fn demo_request_with_content_type(
    path: &str,
    method: &str,
    cookie: Option<&str>,
    scope: Option<&str>,
    content_type: &str,
    body: Vec<u8>,
) -> crate::server::Input {
    let mut input = demo_request(path, method, cookie, scope, body);
    input
        .headers
        .iter_mut()
        .find(|(k, _)| k == "Content-Type")
        .unwrap()
        .1 = content_type.into();
    input
}

fn json_body(reply: &HostedReply) -> Value {
    serde_json::from_slice(&reply.reply.body).unwrap()
}

fn first_demo_company_id(app: &mut Hosted, cookie: &str, scope: &str) -> i64 {
    let list = app.dispatch(
        &demo_request(
            "/api/companies?limit=1",
            "GET",
            Some(cookie),
            Some(scope),
            Vec::new(),
        ),
        AT + 1,
    );
    assert_eq!(list.reply.status, 200);
    json_body(&list)["rows"][0]["id"].as_i64().unwrap()
}

fn assert_demo_public_disabled_routes(app: &mut Hosted, cookie: &str, scope: &str) {
    let public_paths = [
        "/forms/intake.js",
        "/forms/intake.css",
        "/forms/demo/0123456789abcdef0123456789abcdef",
        "/oauth/activity-return",
        "/oauth/activity-return.js",
        "/oauth/activity-return/extra",
    ];
    for prefix in demo::DISABLED_PUBLIC_PREFIXES {
        assert!(
            public_paths.iter().any(|path| path.starts_with(prefix)),
            "untested disabled public prefix: {prefix}"
        );
    }
    for path in public_paths {
        for method in ["GET", "POST"] {
            let body = if method == "POST" {
                b"{}".to_vec()
            } else {
                Vec::new()
            };
            for (auth_cookie, auth_scope) in [(None, None), (Some(cookie), Some(scope))] {
                let reply = app.dispatch(
                    &demo_request(path, method, auth_cookie, auth_scope, body.clone()),
                    AT + 1,
                );
                assert_eq!(reply.reply.status, 404, "{method} {path}");
                assert_eq!(
                    json_body(&reply),
                    json!({"error": "対象が見つかりません。"}),
                    "{method} {path}"
                );
            }
        }
    }
}

const DISABLED_AUTH_SAMPLES: &[(&str, &str, &[u8])] = &[
    (
        "/api/import",
        "text/csv",
        b"origin,name,prefecture\nsynthetic,test,13\n",
    ),
    (
        "/api/import?encoding=utf8&duplicate=error",
        "text/csv",
        b"origin,name,prefecture\nsynthetic,test,13\n",
    ),
    ("/api/leads", "application/json", b"{}"),
    ("/api/leads/1", "application/json", b"{}"),
    (
        "/api/leads/1/assign",
        "application/json",
        b"{\"revision\":1,\"owner_id\":1}",
    ),
    (
        "/api/leads/1/discard",
        "application/json",
        b"{\"revision\":1}",
    ),
    ("/api/leads/1/convert", "application/json", b"{}"),
    ("/api/lead-form", "application/json", b"{\"enabled\":true}"),
    ("/api/activity-sync", "application/json", b"{}"),
    ("/api/activity-sync/google/begin", "application/json", b"{}"),
    (
        "/api/activity-sync/google/finish",
        "application/json",
        b"{\"state\":\"s\",\"code\":\"c\"}",
    ),
    ("/api/activity-sync/google/pull", "application/json", b"{}"),
    (
        "/api/activity-sync/google/disconnect",
        "application/json",
        b"{}",
    ),
    (
        "/api/activity-sync/microsoft/begin",
        "application/json",
        b"{}",
    ),
];

fn assert_demo_auth_disabled_routes(app: &mut Hosted, cookie: &str, scope: &str) {
    for route in demo::DISABLED_AUTH_ROUTES {
        assert!(
            DISABLED_AUTH_SAMPLES.iter().any(|(path, _, _)| path
                .split('?')
                .next()
                .unwrap_or_default()
                == *route),
            "untested disabled auth route: {route}"
        );
    }
    let expected = json!({
        "error": "公開デモでは使えません。",
        "code": "demo_disabled"
    });
    for &(path, content_type, post_body) in DISABLED_AUTH_SAMPLES {
        for method in ["GET", "POST"] {
            let body = if method == "POST" {
                post_body.to_vec()
            } else {
                Vec::new()
            };
            let unauth = app.dispatch(
                &demo_request_with_content_type(
                    path,
                    method,
                    None,
                    None,
                    content_type,
                    body.clone(),
                ),
                AT + 1,
            );
            assert_eq!(unauth.reply.status, 401, "unauth {method} {path}");
            let authed = app.dispatch(
                &demo_request_with_content_type(
                    path,
                    method,
                    Some(cookie),
                    Some(scope),
                    content_type,
                    body,
                ),
                AT + 1,
            );
            assert_eq!(authed.reply.status, 403, "authed {method} {path}");
            assert_eq!(json_body(&authed), expected, "authed {method} {path}");
        }
    }
}

#[test]
fn demo_disables_public_and_authenticated_forbidden_routes_without_changing_normal_mode() {
    let (_temp, mut app) = demo_fixture(None);
    let (cookie, scope, _) = enter(&mut app, AT);
    assert_demo_public_disabled_routes(&mut app, &cookie, &scope);
    assert_demo_auth_disabled_routes(&mut app, &cookie, &scope);

    // 管理者専用経路もデモの編集者権限では403で拒否される。
    for (method, path, body) in [
        ("GET", "/auth/audit", Vec::new()),
        (
            "POST",
            "/api/sales-targets",
            json!({"month":"2026-11","owner_id":1,"amount_jpy":100})
                .to_string()
                .into_bytes(),
        ),
        (
            "POST",
            "/api/companies/1/contacts/1/erase",
            b"{\"wipe_notes\":false}".to_vec(),
        ),
    ] {
        let reply = app.dispatch(
            &demo_request(path, method, Some(&cookie), Some(&scope), body),
            AT + 1,
        );
        assert_eq!(reply.reply.status, 403, "admin-only {method} {path}");
    }

    // 非デモでは従来どおり動作する。
    let (_normal_temp, store) = fixture();
    let mut normal = Hosted::new(store, config());
    let admin = login(&mut normal.store, "alpha");
    for path in ["/forms/intake.js", "/oauth/activity-return"] {
        assert_eq!(
            normal
                .dispatch(&request(path, "GET", None, &json!(null)), AT)
                .reply
                .status,
            200
        );
    }
    for path in ["/api/leads", "/api/lead-form", "/api/activity-sync"] {
        assert_eq!(
            normal
                .dispatch(&request(path, "GET", Some(&admin), &json!(null)), AT)
                .reply
                .status,
            200
        );
    }
    let mut import_req = request("/api/import", "POST", Some(&admin), &json!(null));
    import_req
        .headers
        .iter_mut()
        .find(|(k, _)| k == "Content-Type")
        .unwrap()
        .1 = "text/csv".into();
    import_req.body = b"origin,name,prefecture\nsynthetic,normal-csv-company,13\n".to_vec();
    assert_eq!(normal.dispatch(&import_req, AT).reply.status, 200);
}

#[test]
fn demo_enforces_64kib_post_body_limit_and_rejects_import_under_limit() {
    let (_temp, mut app) = demo_fixture(None);
    let (cookie, scope, _) = enter(&mut app, AT);

    let payload = r#"{"name":"合成・上限確認株式会社","prefecture":13}"#.as_bytes();
    let mut exact = vec![b' '; demo::MAX_POST_BODY_BYTES - payload.len()];
    exact.extend_from_slice(payload);
    assert_eq!(exact.len(), demo::MAX_POST_BODY_BYTES);
    let ok = app.dispatch(
        &demo_request("/api/companies", "POST", Some(&cookie), Some(&scope), exact),
        AT + 1,
    );
    assert_eq!(ok.reply.status, 201);

    let mut over = vec![b' '; demo::MAX_POST_BODY_BYTES + 1 - payload.len()];
    over.extend_from_slice(payload);
    assert_eq!(over.len(), demo::MAX_POST_BODY_BYTES + 1);
    let too_large = app.dispatch(
        &demo_request("/api/companies", "POST", Some(&cookie), Some(&scope), over),
        AT + 2,
    );
    assert_eq!(too_large.reply.status, 413);

    // /demo/enter は257バイトで413。
    let mut enter_over = vec![b' '; 255];
    enter_over.extend_from_slice(b"{}");
    assert_eq!(enter_over.len(), 257);
    assert_eq!(
        app.dispatch(
            &demo_request("/demo/enter", "POST", None, None, enter_over),
            AT + 3,
        )
        .reply
        .status,
        413
    );

    // /api/import は64KiB以下でも403（demo_disabled）、64KiB+1は413。
    let csv_ok = b"origin,name,prefecture\nsynthetic,demo-import,13\n".to_vec();
    let import_disabled = app.dispatch(
        &demo_request_with_content_type(
            "/api/import",
            "POST",
            Some(&cookie),
            Some(&scope),
            "text/csv",
            csv_ok,
        ),
        AT + 4,
    );
    assert_eq!(import_disabled.reply.status, 403);
    assert_eq!(json_body(&import_disabled)["code"], "demo_disabled");

    let csv_over = vec![b'a'; demo::MAX_POST_BODY_BYTES + 1];
    let import_over = app.dispatch(
        &demo_request_with_content_type(
            "/api/import",
            "POST",
            Some(&cookie),
            Some(&scope),
            "text/csv",
            csv_over,
        ),
        AT + 5,
    );
    assert_eq!(import_over.reply.status, 413);

    // 通常モードでは64KiB+1（1MiB未満）のPOSTが引き続き通る。
    let (_normal_temp, store) = fixture();
    let mut normal = Hosted::new(store, config());
    let admin = login(&mut normal.store, "alpha");
    let mut normal_req = request("/api/companies", "POST", Some(&admin), &json!(null));
    let mut normal_over_64k = vec![b' '; demo::MAX_POST_BODY_BYTES + 1 - payload.len()];
    normal_over_64k.extend_from_slice(payload);
    normal_req.body = normal_over_64k;
    assert_eq!(normal.dispatch(&normal_req, AT).reply.status, 201);
}

#[test]
fn demo_post_preflight_checks_content_length_expect_and_body_size() {
    let empty: Vec<(String, String)> = Vec::new();
    let chunked = vec![("Transfer-Encoding".into(), "chunked".into())];
    let expect = vec![("Expect".into(), "100-continue".into())];
    let expect_lower = vec![("expect".into(), "100-continue".into())];
    let normal = vec![("Content-Length".into(), "10".into())];

    assert_eq!(
        demo::check_post_preflight("POST", &empty, None, 65_536).map(|(s, _)| s),
        Some(411)
    );
    assert_eq!(
        demo::check_post_preflight("POST", &chunked, None, 65_536).map(|(s, _)| s),
        Some(411)
    );
    assert_eq!(
        demo::check_post_preflight("POST", &chunked, Some(10), 65_536).map(|(s, _)| s),
        Some(411)
    );
    assert_eq!(
        demo::check_post_preflight("POST", &expect, Some(10), 65_536).map(|(s, _)| s),
        Some(417)
    );
    assert_eq!(
        demo::check_post_preflight("POST", &expect_lower, None, 65_536).map(|(s, _)| s),
        Some(417)
    );
    assert_eq!(
        demo::check_post_preflight("GET", &expect, None, 65_536).map(|(s, _)| s),
        Some(417)
    );
    assert_eq!(
        demo::check_post_preflight("POST", &normal, Some(65_537), 65_536).map(|(s, _)| s),
        Some(413)
    );
    assert_eq!(
        demo::check_post_preflight("POST", &normal, Some(65_536), 65_536),
        None
    );
    assert_eq!(
        demo::check_post_preflight("POST", &normal, Some(0), 256),
        None
    );
    assert_eq!(
        demo::check_post_preflight("GET", &empty, None, 65_536),
        None
    );
}

fn demo_post_json(
    app: &mut Hosted,
    cookie: &str,
    scope: &str,
    path: &str,
    body: &Value,
) -> HostedReply {
    app.dispatch(
        &demo_request(
            path,
            "POST",
            Some(cookie),
            Some(scope),
            body.to_string().into_bytes(),
        ),
        AT + 1,
    )
}

fn assert_demo_limit_reply(reply: &HostedReply, label: &str) {
    assert_eq!(reply.reply.status, 403, "{label}");
    assert_eq!(
        json_body(reply),
        json!({
            "error": "デモの上限に達しました。しばらくすると初期化されます。",
            "code": "demo_limit"
        }),
        "{label}"
    );
}

fn assert_company_and_activity_limits(
    app: &mut Hosted,
    cookie: &str,
    scope: &str,
    company_id: i64,
) {
    // 取引先: 151社目は成功、152社目は403、更新は200。
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            "/api/companies",
            &json!({"name":"合成・追加テスト株式会社","prefecture":13}),
        )
        .reply
        .status,
        201
    );
    assert_demo_limit_reply(
        &demo_post_json(
            app,
            cookie,
            scope,
            "/api/companies",
            &json!({"name":"合成・上限超過株式会社","prefecture":13}),
        ),
        "company create limit",
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/companies/{company_id}"),
            &json!({"name":"合成・更新テスト株式会社","prefecture":13,"municipality":null}),
        )
        .reply
        .status,
        200
    );

    // 活動: 通常 + /api/v1 + 取消1件 で上限4に到達。以降の通常・再架電・v1・取消はすべて403。
    let act_path = format!("/api/companies/{company_id}/activities");
    let v1_act_path = format!("/api/v1/companies/{company_id}/activities");
    assert_eq!(
        demo_post_json(app, cookie, scope, &act_path, &json!({"note":"合成メモ1"}))
            .reply
            .status,
        200
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &v1_act_path,
            &json!({"note":"合成メモ2"})
        )
        .reply
        .status,
        200
    );
    let detail = app.dispatch(
        &demo_request(
            &format!("/api/companies/{company_id}"),
            "GET",
            Some(cookie),
            Some(scope),
            Vec::new(),
        ),
        AT + 1,
    );
    let activities = json_body(&detail)["activities"].as_array().unwrap().clone();
    assert!(activities.len() >= 2);
    let first_activity_id = activities[0]["id"].as_i64().unwrap();
    let second_activity_id = activities[1]["id"].as_i64().unwrap();
    // 上限到達前の取消は成功し、活動件数を1つ消費する（これで計4件）。
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/companies/{company_id}/activities/{first_activity_id}/cancel"),
            &json!({}),
        )
        .reply
        .status,
        200
    );
    let cancel_over_path =
        format!("/api/companies/{company_id}/activities/{second_activity_id}/cancel");
    for (path, payload) in [
        (act_path.as_str(), json!({"note":"合成メモ上限超過"})),
        (
            act_path.as_str(),
            json!({"note":"不在","follow_up":"absence","request_key":"demo-limit-absence-0001"}),
        ),
        (v1_act_path.as_str(), json!({"note":"v1上限超過"})),
        (cancel_over_path.as_str(), json!({})),
    ] {
        assert_demo_limit_reply(&demo_post_json(app, cookie, scope, path, &payload), path);
    }
}

fn assert_opportunity_and_field_limits(
    app: &mut Hosted,
    cookie: &str,
    scope: &str,
    company_id: i64,
) {
    // 商談: 10件目は成功、11件目は403、更新と段階変更は200。
    let deal = json!({
        "company_id": company_id,
        "name": "合成・追加商談",
        "amount_jpy": 100_000,
        "stage": 1,
        "close_date": "2026-12-20"
    });
    let created_deal = demo_post_json(app, cookie, scope, "/api/opportunities", &deal);
    assert_eq!(created_deal.reply.status, 201);
    let deal_id = json_body(&created_deal)["id"].as_i64().unwrap();
    assert_demo_limit_reply(
        &demo_post_json(app, cookie, scope, "/api/opportunities", &deal),
        "opportunity create limit",
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/opportunities/{deal_id}"),
            &deal
        )
        .reply
        .status,
        200
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/opportunities/{deal_id}/stage"),
            &json!({"stage": 2}),
        )
        .reply
        .status,
        200
    );

    // 独自項目: 1件目は成功、2件目は403、値の設定・検索・削除は200。
    let created_field = demo_post_json(
        app,
        cookie,
        scope,
        "/api/custom-fields",
        &json!({"name":"合成区分","kind":"text","indexed":true}),
    );
    assert_eq!(created_field.reply.status, 201);
    let field_id = json_body(&created_field)["id"].as_i64().unwrap();
    assert_demo_limit_reply(
        &demo_post_json(
            app,
            cookie,
            scope,
            "/api/custom-fields",
            &json!({"name":"合成区分2","kind":"text","indexed":false}),
        ),
        "custom field create limit",
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/companies/{company_id}/custom-fields"),
            &json!({"field_id": field_id, "value": "A"}),
        )
        .reply
        .status,
        200
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/custom-fields/{field_id}/search"),
            &json!({"value": "A"}),
        )
        .reply
        .status,
        200
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/custom-fields/{field_id}/delete"),
            &json!({}),
        )
        .reply
        .status,
        200
    );
}

fn assert_contact_work_and_list_limits(
    app: &mut Hosted,
    cookie: &str,
    scope: &str,
    company_id: i64,
) {
    // 取引先の連絡先: 1件目は成功、2件目は403、更新は200。
    let contacts_path = format!("/api/companies/{company_id}/contacts");
    let created_contact = demo_post_json(
        app,
        cookie,
        scope,
        &contacts_path,
        &json!({"name":"合成 担当A"}),
    );
    assert_eq!(created_contact.reply.status, 201);
    let contact_id = json_body(&created_contact)["id"].as_i64().unwrap();
    assert_demo_limit_reply(
        &demo_post_json(
            app,
            cookie,
            scope,
            &contacts_path,
            &json!({"name":"合成 担当B"}),
        ),
        "contact create limit",
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("{contacts_path}/{contact_id}"),
            &json!({"name":"合成 担当A更新"}),
        )
        .reply
        .status,
        200
    );

    // 仕事（ToDo・予定）: 2件目は成功、3件目は403、更新と完了は200。
    let work = json!({
        "company_id": company_id,
        "kind": "task",
        "subject": "合成タスク",
        "due_at": AT + 3600
    });
    let created_work = demo_post_json(app, cookie, scope, "/api/work-items", &work);
    assert_eq!(created_work.reply.status, 201);
    let work_id = json_body(&created_work)["id"].as_i64().unwrap();
    assert_demo_limit_reply(
        &demo_post_json(app, cookie, scope, "/api/work-items", &work),
        "work item create limit",
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/work-items/{work_id}"),
            &work
        )
        .reply
        .status,
        200
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/work-items/{work_id}/completion"),
            &json!({"completed": true}),
        )
        .reply
        .status,
        200
    );
}

fn assert_calling_list_limits(app: &mut Hosted, cookie: &str, scope: &str) {
    // 共有架電リスト: 1件目は成功、2件目は403、更新と削除は200。
    let list = json!({"name": "合成リスト", "conditions": {"prefecture": "13"}});
    let created_list = demo_post_json(app, cookie, scope, "/api/calling-lists", &list);
    assert_eq!(created_list.reply.status, 201);
    let list_id = json_body(&created_list)["id"].as_u64().unwrap();
    assert_demo_limit_reply(
        &demo_post_json(app, cookie, scope, "/api/calling-lists", &list),
        "calling list create limit",
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/calling-lists/{list_id}"),
            &json!({"name": "合成リスト更新", "conditions": {"prefecture": "13"}, "revision": 1}),
        )
        .reply
        .status,
        200
    );
    assert_eq!(
        demo_post_json(
            app,
            cookie,
            scope,
            &format!("/api/calling-lists/{list_id}/delete"),
            &json!({"revision": 2}),
        )
        .reply
        .status,
        200
    );
}

#[test]
fn demo_enforces_creation_count_limits_including_activity_cancel_while_allowing_updates_and_completions()
 {
    let temp = tempfile::tempdir().unwrap();
    let store = demo::bootstrap(&temp.path().join("demo-limits"), AT).unwrap();
    let mut cfg = demo_config(None);
    let demo_cfg = cfg.demo.as_mut().unwrap();
    // 初期件数: 取引先150、活動1、商談9、独自項目0、担当者0、仕事1、共有リスト0。
    demo_cfg.max_companies = 151;
    demo_cfg.max_activities = 4;
    demo_cfg.max_opportunities = 10;
    demo_cfg.max_custom_fields = 1;
    demo_cfg.max_contacts = 1;
    demo_cfg.max_work_items = 2;
    demo_cfg.max_calling_lists = 1;
    let mut app = Hosted::new(store, cfg);
    let (cookie, scope, _) = enter(&mut app, AT);
    let company_id = first_demo_company_id(&mut app, &cookie, &scope);

    assert_company_and_activity_limits(&mut app, &cookie, &scope, company_id);
    assert_opportunity_and_field_limits(&mut app, &cookie, &scope, company_id);
    assert_contact_work_and_list_limits(&mut app, &cookie, &scope, company_id);
    assert_calling_list_limits(&mut app, &cookie, &scope);
}

#[test]
fn demo_enforces_storage_limit_on_write_posts_while_allowing_reads_logout_and_enter() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("demo-storage");
    let store = demo::bootstrap(&root, AT).unwrap();
    // 検索専用の読み取りPOSTを確認するため、独自項目を1件だけ先に作っておく。
    let field_id = {
        let mut engine = Engine::open(
            &store.tenant_path(demo::DEMO_TENANT).unwrap(),
            &OpenOptions::default(),
        )
        .unwrap();
        engine
            .create_custom_field(&meibo_core::custom_fields::CustomFieldInput {
                name: "合成検索用".into(),
                kind: meibo_core::custom_fields::CustomFieldKind::Text,
                indexed: true,
            })
            .unwrap()
    };

    let base_bytes = demo::storage_bytes(&root).unwrap();
    assert!(base_bytes > 0);
    // 配下のWAL相当ファイルも合計サイズに含まれることを確認する。
    let extra_file = root.join("tenants/demo.sqlite3-extra-check");
    std::fs::write(&extra_file, vec![0_u8; 4096]).unwrap();
    assert_eq!(demo::storage_bytes(&root).unwrap(), base_bytes + 4096);
    std::fs::remove_file(&extra_file).unwrap();

    let mut cfg = demo_config(None);
    cfg.demo.as_mut().unwrap().max_storage_bytes = 1;
    let mut app = Hosted::new(store, cfg);

    // 保存量上限に達していても /demo/enter は通る。
    let (cookie, scope, _) = enter(&mut app, AT);
    let company_id = first_demo_company_id(&mut app, &cookie, &scope);

    // 読み取りGETと読み取り専用の独自項目検索POSTは通る。
    for path in ["/api/summary", "/api/companies", "/auth/session"] {
        let reply = app.dispatch(
            &demo_request(path, "GET", Some(&cookie), Some(&scope), Vec::new()),
            AT + 1,
        );
        assert_eq!(reply.reply.status, 200, "read {path} should succeed");
    }
    let search = app.dispatch(
        &demo_request(
            &format!("/api/custom-fields/{field_id}/search"),
            "POST",
            Some(&cookie),
            Some(&scope),
            json!({"value": "x"}).to_string().into_bytes(),
        ),
        AT + 1,
    );
    assert_eq!(search.reply.status, 200);

    // 書き込みPOSTは経路を問わず507（demo_storage_full）になる。
    let expected_full = json!({
        "error": "デモの保存量の上限に達しました。しばらくすると初期化されます。",
        "code": "demo_storage_full"
    });
    for (path, payload) in [
        (
            "/api/companies".to_owned(),
            json!({"name":"合成・容量超過","prefecture":13}),
        ),
        (
            format!("/api/companies/{company_id}"),
            json!({"name":"合成・容量超過更新","prefecture":13}),
        ),
        (
            format!("/api/companies/{company_id}/activities"),
            json!({"note":"合成メモ"}),
        ),
        ("/api/opportunities/1/stage".to_owned(), json!({"stage": 3})),
        (
            format!("/api/companies/{company_id}/custom-fields"),
            json!({"field_id": field_id, "value": "A"}),
        ),
        (
            "/api/calling-lists".to_owned(),
            json!({"name": "合成リスト", "conditions": {"prefecture": "13"}}),
        ),
    ] {
        let reply = app.dispatch(
            &demo_request(
                &path,
                "POST",
                Some(&cookie),
                Some(&scope),
                payload.to_string().into_bytes(),
            ),
            AT + 2,
        );
        assert_eq!(reply.reply.status, 507, "write POST {path} should be 507");
        assert_eq!(json_body(&reply), expected_full, "write POST {path}");
    }

    // /auth/logout は保存量上限でも通る。
    let logout = app.dispatch(
        &demo_request(
            "/auth/logout",
            "POST",
            Some(&cookie),
            Some(&scope),
            b"{}".to_vec(),
        ),
        AT + 3,
    );
    assert_eq!(logout.reply.status, 200);
}

fn parse_demo_args(extra: &[&str]) -> HostedResult<HostedConfig> {
    let mut args = vec![
        "--origin".to_owned(),
        "https://demo.example.invalid".to_owned(),
        "--public-demo".to_owned(),
    ];
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    cli::serve_config(&args)
}

#[test]
fn demo_cli_timeout_options_validate_defaults_ranges_duplicates_and_require_public_demo() {
    let defaults = parse_demo_args(&[]).unwrap().demo.unwrap();
    assert_eq!(defaults.idle_timeout, Duration::from_secs(900));
    assert_eq!(defaults.max_uptime, Duration::from_secs(21_600));
    assert_eq!(defaults.stall_timeout, Duration::from_secs(10));

    let min_cfg = parse_demo_args(&[
        "--demo-idle-seconds",
        "1",
        "--demo-max-uptime-seconds",
        "1",
        "--demo-stall-seconds",
        "1",
    ])
    .unwrap()
    .demo
    .unwrap();
    assert_eq!(min_cfg.idle_timeout, Duration::from_secs(1));
    assert_eq!(min_cfg.max_uptime, Duration::from_secs(1));
    assert_eq!(min_cfg.stall_timeout, Duration::from_secs(1));

    let max_cfg = parse_demo_args(&[
        "--demo-idle-seconds",
        "86400",
        "--demo-max-uptime-seconds",
        "86400",
        "--demo-stall-seconds",
        "600",
    ])
    .unwrap()
    .demo
    .unwrap();
    assert_eq!(max_cfg.idle_timeout, Duration::from_secs(86_400));
    assert_eq!(max_cfg.max_uptime, Duration::from_secs(86_400));
    assert_eq!(max_cfg.stall_timeout, Duration::from_secs(600));

    for (flag, bad_values) in [
        (
            "--demo-idle-seconds",
            &["0", "86401", "-1", "+10", "abc", ""][..],
        ),
        (
            "--demo-max-uptime-seconds",
            &["0", "86401", "-1", "+10", "abc", ""][..],
        ),
        (
            "--demo-stall-seconds",
            &["0", "601", "-1", "+10", "abc", ""][..],
        ),
    ] {
        for bad in bad_values {
            let err = parse_demo_args(&[flag, bad]).err().unwrap().to_string();
            assert!(err.contains(flag), "{flag}={bad}: {err}");
        }
        assert!(parse_demo_args(&[flag]).is_err(), "missing value: {flag}");
        assert!(
            parse_demo_args(&[flag, "10", flag, "20"]).is_err(),
            "duplicate flag: {flag}"
        );
        let solo_err = cli::serve_config(&[
            "--origin".into(),
            "https://crm.example.invalid".into(),
            flag.into(),
            "10".into(),
        ])
        .err()
        .unwrap()
        .to_string();
        assert!(
            solo_err.contains("--public-demo"),
            "solo {flag}: {solo_err}"
        );
    }
}
