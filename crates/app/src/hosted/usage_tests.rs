//! usage-dashboard against loopback sqld: grouping, saving only on the
//! serialized path, permissions, a missing table, and backup/restore.
use meibo_core::types::DataOrigin;
use meibo_db::{Connection, RemoteOptions};
use serde_json::{Value, json};

use super::{
    Hosted, HostedConfig, HostedReply, Login, LoginOutcome, Role,
    remote::{
        config::{EndpointPolicy, Endpoints, RemoteConfig},
        operation::RemoteOperation,
        store::RemoteStore,
    },
    usage::{Counts, Group, UsageState},
};
use crate::server::Input;

#[allow(dead_code)]
mod sqld {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/sqld.rs"
    ));
}

const PASSWORD: &str = "synthetic-usage-password-only";
const TOKEN: &str = "synthetic-usage-token-only";
const ADMIN: &str = "synthetic-usage-owner";
const AT: i64 = 1_800_000_000;

fn endpoints(control: &str, tenant: &str) -> Endpoints {
    RemoteConfig::parse(br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"}}}"#).unwrap()
        .resolve_with(|key| Some(if key.ends_with("TOKEN") { TOKEN.into() } else if key == "MEIBO_DB_CONTROL_URL" { control.into() } else { tenant.into() }), EndpointPolicy::LoopbackTests).unwrap()
}

struct Fixture {
    control: sqld::Sqld,
    tenant: sqld::Sqld,
}

impl Fixture {
    fn new() -> Self {
        let control = sqld::Sqld::start().unwrap();
        let tenant = sqld::Sqld::start().unwrap();
        let mut store =
            RemoteStore::initialize(endpoints(&control.endpoint(), &tenant.endpoint())).unwrap();
        store
            .add_empty_tenant("alpha", DataOrigin::Synthetic)
            .unwrap();
        for (name, role) in [
            (ADMIN, Role::Admin),
            ("synthetic-usage-editor", Role::Editor),
            ("synthetic-usage-viewer", Role::Viewer),
        ] {
            store.add_user(name, PASSWORD).unwrap();
            store.set_member(name, "alpha", Some(role)).unwrap();
        }
        Self { control, tenant }
    }
    fn endpoints(&self) -> Endpoints {
        endpoints(&self.control.endpoint(), &self.tenant.endpoint())
    }
    fn store(&self) -> RemoteStore {
        RemoteStore::open(self.endpoints()).unwrap()
    }
    fn login(&self, name: &str) -> Login {
        let LoginOutcome::Success(login) = self.store().login("alpha", name, PASSWORD, AT).unwrap()
        else {
            panic!("synthetic login");
        };
        login
    }
    fn service(&self) -> Hosted<RemoteStore> {
        let mut web = Hosted::new(
            self.store(),
            HostedConfig::parse("https://crm.example.test", "127.0.0.1:0", false).unwrap(),
        );
        web.start_usage(Some(1000)).unwrap();
        web
    }
    fn raw_control(&self) -> Connection {
        Connection::open_remote(
            RemoteOptions::new(self.control.endpoint(), TOKEN).allow_loopback_http_for_tests(),
        )
        .unwrap()
    }
    fn saved(&self) -> Vec<(String, String, i64, i64)> {
        let conn = self.raw_control();
        let mut statement = conn
            .prepare("SELECT day,route,requests,round_trips FROM usage_daily ORDER BY day,route")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
    fn denied_audits(&self) -> i64 {
        self.raw_control()
            .query_row(
                "SELECT count(*) FROM audit WHERE operation='authorization' AND outcome='denied'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
}

fn request(login: Option<&Login>, method: &str, target: &str, value: &Value) -> Input {
    let mut headers = vec![
        ("Host".into(), "crm.example.test".into()),
        ("Origin".into(), "https://crm.example.test".into()),
        ("Content-Type".into(), "application/json".into()),
        ("X-Meibo-Local".into(), "1".into()),
    ];
    if let Some(login) = login {
        headers.push((
            "Cookie".into(),
            format!("__Host-meibo-session={}", login.token),
        ));
        headers.push(("X-Meibo-Context".into(), login.identity.scope.clone()));
    }
    Input {
        method: method.into(),
        target: target.into(),
        body: if method == "POST" {
            value.to_string().into_bytes()
        } else {
            vec![]
        },
        headers,
    }
}

fn body(reply: &HostedReply) -> Value {
    serde_json::from_slice(&reply.reply.body).unwrap()
}

fn pending(web: &Hosted<RemoteStore>, group: Group) -> Counts {
    let UsageState::On(usage) = &web.usage else {
        panic!("usage is on");
    };
    usage.pending.iter().filter(|((_, g), _)| *g == group).fold(
        Counts::default(),
        |mut total, (_, counts)| {
            total.requests += counts.requests;
            total.round_trips += counts.round_trips;
            total.rows_written += counts.rows_written;
            total
        },
    )
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    // Shared with the backup fixtures: concurrent sqld starts can take the
    // same released ephemeral port.
    super::remote_backup::tests::integration::FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn counts_are_saved_only_on_the_serialized_path_once_a_minute() {
    let _serial = serial();
    let fixture = Fixture::new();
    let admin = fixture.login(ADMIN);
    let mut web = fixture.service();
    let company = json!({"name":"合成 利用量 株式会社","prefecture":13});
    // Fast path: nothing is written and the window starts here.
    let reply = web.dispatch(
        &request(
            Some(&admin),
            "GET",
            "/api/companies?q=synthetic-secret-term",
            &Value::Null,
        ),
        AT + 10,
    );
    assert_eq!(reply.reply.status, 200);
    assert_eq!(pending(&web, Group::List).requests, 1);
    assert!(pending(&web, Group::List).round_trips > 0);
    // Serialized, 50 s later: not due yet. It also refreshes last_seen.
    let reply = web.dispatch(
        &request(Some(&admin), "POST", "/api/companies", &company),
        AT + 60,
    );
    assert_eq!(reply.reply.status, 201);
    assert_eq!(fixture.saved().len(), 0);
    // Due, but a gate-free read never writes (1.3).
    let reply = web.dispatch(
        &request(Some(&admin), "GET", "/api/summary", &Value::Null),
        AT + 70,
    );
    assert_eq!(reply.reply.status, 200);
    assert_eq!(fixture.saved().len(), 0);
    // The next serialized request writes everything pending before it.
    let reply = web.dispatch(
        &request(Some(&admin), "POST", "/api/companies", &company),
        AT + 71,
    );
    assert_eq!(reply.reply.status, 201);
    let saved = fixture.saved();
    let routes: Vec<&str> = saved.iter().map(|row| row.1.as_str()).collect();
    assert_eq!(routes, ["list", "write"]);
    assert_eq!(saved.iter().map(|row| row.2).sum::<i64>(), 3);
    // The flush's own round trips are not counted (1.6): two identical
    // writes, one with a flush and one without, record the same round trips.
    let with_flush = pending(&web, Group::Write);
    assert_eq!(with_flush.requests, 1);
    let reply = web.dispatch(
        &request(Some(&admin), "POST", "/api/companies", &company),
        AT + 72,
    );
    assert_eq!(reply.reply.status, 201);
    let both = pending(&web, Group::Write);
    assert_eq!(both.requests, 2);
    assert_eq!(
        both.round_trips - with_flush.round_trips,
        with_flush.round_trips
    );
    // Only route keys and counts are stored (1.2).
    let dump = format!("{:?}", fixture.saved());
    for secret in [
        ADMIN,
        "alpha",
        "synthetic-secret-term",
        admin.token.as_str(),
        "合成",
    ] {
        assert!(!dump.contains(secret));
    }
    // The admin report merges saved and pending counts, and only reads.
    let reply = web.dispatch(
        &request(Some(&admin), "GET", "/api/admin/usage", &Value::Null),
        AT + 73,
    );
    assert_eq!(reply.reply.status, 200);
    assert_eq!(reply.header("Cache-Control"), None, "default no-store");
    let report = body(&reply);
    assert_eq!(report["configured"], true);
    assert_eq!(report["totals"]["requests"], 5);
    assert_eq!(report["unsaved_requests"], 2);
    assert_eq!(report["cap_jpy"], 1000);
    assert_eq!(report["days"].as_array().unwrap().len(), 31);
    let text = String::from_utf8(reply.reply.body.clone()).unwrap();
    for secret in [
        ADMIN,
        "\"alpha\"",
        "synthetic-secret-term",
        admin.token.as_str(),
        "合成",
    ] {
        assert!(!text.contains(secret), "report leaks {secret}");
    }
    assert_eq!(fixture.saved().iter().map(|row| row.2).sum::<i64>(), 3);
    // The admin entry appears on the business screen.
    let reply = web.dispatch(&request(Some(&admin), "GET", "/", &Value::Null), AT + 74);
    assert_eq!(reply.reply.status, 200);
    assert!(String::from_utf8_lossy(&reply.reply.body).contains("href=\"/usage\""));
}

#[test]
fn only_admins_see_the_usage_page_and_data_and_denials_are_audited() {
    let _serial = serial();
    let fixture = Fixture::new();
    let admin = fixture.login(ADMIN);
    let mut web = fixture.service();
    let page = web.dispatch(
        &request(Some(&admin), "GET", "/usage", &Value::Null),
        AT + 1,
    );
    assert_eq!(page.reply.status, 200);
    assert!(String::from_utf8_lossy(&page.reply.body).contains("利用量と費用の目安"));
    for asset in ["/usage.js", "/usage.css"] {
        let reply = web.dispatch(&request(Some(&admin), "GET", asset, &Value::Null), AT + 1);
        assert_eq!(reply.reply.status, 200, "{asset}");
    }
    let before = fixture.denied_audits();
    let mut denied = 0;
    for name in ["synthetic-usage-editor", "synthetic-usage-viewer"] {
        let login = fixture.login(name);
        for target in ["/api/admin/usage", "/usage"] {
            let reply = web.dispatch(&request(Some(&login), "GET", target, &Value::Null), AT + 2);
            assert_eq!(reply.reply.status, 403, "{name} {target}");
            // A browser gets a page; the API keeps its JSON error.
            let html = reply.reply.mime.starts_with("text/html");
            assert_eq!(html, target == "/usage", "{name} {target}");
            if html {
                assert!(
                    String::from_utf8_lossy(&reply.reply.body)
                        .contains("管理者だけが開ける画面です")
                );
            }
            denied += 1;
        }
        let home = web.dispatch(&request(Some(&login), "GET", "/", &Value::Null), AT + 2);
        assert_eq!(home.reply.status, 200);
        assert!(!String::from_utf8_lossy(&home.reply.body).contains("/usage"));
        // The stylesheet of the denied page has no data and is open to members.
        let css = web.dispatch(
            &request(Some(&login), "GET", "/usage.css", &Value::Null),
            AT + 2,
        );
        assert_eq!(css.reply.status, 200);
        let script = web.dispatch(
            &request(Some(&login), "GET", "/usage.js", &Value::Null),
            AT + 2,
        );
        assert_eq!(script.reply.status, 403);
        denied += 1;
    }
    assert_eq!(fixture.denied_audits() - before, denied);
    // A POST is refused even for an admin.
    let reply = web.dispatch(
        &request(Some(&admin), "POST", "/api/admin/usage", &json!({})),
        AT + 3,
    );
    assert_eq!(reply.reply.status, 403);
    // Without a session the page sends the browser to the login page.
    let reply = web.dispatch(&request(None, "GET", "/usage", &Value::Null), AT + 3);
    assert_eq!(reply.reply.status, 302);
    assert_eq!(reply.header("Location"), Some("/login"));
    let reply = web.dispatch(
        &request(None, "GET", "/api/admin/usage", &Value::Null),
        AT + 3,
    );
    assert_eq!(reply.reply.status, 401);
    // The data needs the same context header as every other API.
    let mut stale = request(Some(&admin), "GET", "/api/admin/usage", &Value::Null);
    stale.headers.retain(|(name, _)| name != "X-Meibo-Context");
    assert_eq!(web.dispatch(&stale, AT + 4).reply.status, 409);
    let groups = body(&web.dispatch(
        &request(Some(&admin), "GET", "/api/admin/usage", &Value::Null),
        AT + 5,
    ));
    let keys: Vec<&str> = groups["groups"]["month"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["route"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"denied") && keys.contains(&"admin") && keys.contains(&"ui"));
}

#[test]
fn usage_enable_live() {
    let _serial = serial();
    let fixture = Fixture::new();
    fixture
        .raw_control()
        .execute_batch("DROP TABLE usage_daily")
        .unwrap();
    let admin = fixture.login(ADMIN);
    let mut web = fixture.service();
    assert!(matches!(web.usage, UsageState::Missing(Some(1000))));
    let reply = web.dispatch(
        &request(Some(&admin), "GET", "/api/admin/usage", &Value::Null),
        AT + 1,
    );
    assert_eq!(body(&reply), json!({"configured": false}));
    let reply = web.dispatch(
        &request(
            Some(&admin),
            "POST",
            "/api/companies",
            &json!({"name":"合成 未設定","prefecture":13}),
        ),
        AT + 100,
    );
    assert_eq!(reply.reply.status, 201);
    // The readiness check is unchanged without the table.
    fixture.store().check_ready().unwrap();
    let store = fixture.store();
    for _ in 0..2 {
        let operation = RemoteOperation::begin(&store, "cli.usage-enable").unwrap();
        let result = store.enable_usage();
        operation.finish(&store, result).unwrap();
    }
    assert!(store.gate_status().unwrap().is_none());
    assert_eq!(fixture.saved().len(), 0);
    let viewer = fixture.login("synthetic-usage-viewer");
    assert_eq!(
        web.dispatch(
            &request(Some(&viewer), "GET", "/api/admin/usage", &Value::Null),
            AT + 101
        )
        .reply
        .status,
        403
    );
    assert!(matches!(web.usage, UsageState::Missing(_)));
    let page = web.dispatch(
        &request(Some(&admin), "GET", "/usage", &Value::Null),
        AT + 101,
    );
    assert!(String::from_utf8_lossy(&page.reply.body).contains("「最新にする」"));
    assert!(matches!(web.usage, UsageState::Missing(_)));
    let report = body(&web.dispatch(
        &request(Some(&admin), "GET", "/api/admin/usage", &Value::Null),
        AT + 101,
    ));
    assert_eq!(report["configured"], true);
    assert_eq!(report["cap_jpy"], 1000);
    assert_eq!(pending(&web, Group::Admin).requests, 1);
    assert_eq!(pending(&web, Group::Admin).rows_written, 0);
    assert_eq!(fixture.saved().len(), 0);
    let report = body(&web.dispatch(
        &request(Some(&admin), "GET", "/api/admin/usage", &Value::Null),
        AT + 102,
    ));
    assert_eq!(report["unsaved_requests"], 1);
    assert!(matches!(fixture.service().usage, UsageState::On(_)));
}

#[test]
fn received_once() {
    use tiny_http::{Header, Method, TestRequest};

    let _serial = serial();
    let fixture = Fixture::new();
    let mut web = fixture.service();
    let valid =
        || TestRequest::new().with_header(Header::from_bytes("Host", "crm.example.test").unwrap());
    let post = || {
        valid()
            .with_method(Method::Post)
            .with_header(Header::from_bytes("Origin", "https://crm.example.test").unwrap())
            .with_header(Header::from_bytes("X-Meibo-Local", "1").unwrap())
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
            .with_path("/auth/login")
    };
    for (index, (request, status)) in [
        (valid().with_method(Method::Head), 405),
        (TestRequest::new(), 403),
        (valid().with_path("//invalid"), 400),
        (
            post().with_header(Header::from_bytes("Content-Length", "8193").unwrap()),
            413,
        ),
        (
            post()
                .with_header(Header::from_bytes("Transfer-Encoding", "chunked").unwrap())
                .with_body("bad chunk\r\n"),
            400,
        ),
        (valid().with_path("/login"), 200),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            web.receive(&mut request.into(), super::http::RequestTime::new(AT))
                .reply
                .status,
            status
        );
        let UsageState::On(usage) = &web.usage else {
            panic!("usage is on")
        };
        assert_eq!(usage.pending_requests(), u64::try_from(index + 1).unwrap());
    }
    assert_eq!(pending(&web, Group::Denied).requests, 5);
    assert_eq!(pending(&web, Group::Denied).round_trips, 0);
    assert_eq!(fixture.saved().len(), 0);
}

#[test]
fn a_failed_save_that_is_not_communication_is_dropped_without_changing_the_reply() {
    let _serial = serial();
    let fixture = Fixture::new();
    let admin = fixture.login(ADMIN);
    let mut web = fixture.service();
    let company = json!({"name":"合成 保存失敗","prefecture":13});
    assert_eq!(
        web.dispatch(
            &request(Some(&admin), "POST", "/api/companies", &company),
            AT + 1
        )
        .reply
        .status,
        201
    );
    fixture
        .raw_control()
        .execute_batch("DROP TABLE usage_daily")
        .unwrap();
    let reply = web.dispatch(
        &request(Some(&admin), "POST", "/api/companies", &company),
        AT + 61,
    );
    assert_eq!(reply.reply.status, 201);
    assert!(!web.store.conn.is_poisoned());
    // Only the current request is pending; the failed batch was dropped.
    assert_eq!(pending(&web, Group::Write).requests, 1);
    assert!(fixture.store().gate_status().unwrap().is_none());
    let reply = web.dispatch(
        &request(Some(&admin), "GET", "/api/summary", &Value::Null),
        AT + 62,
    );
    assert_eq!(reply.reply.status, 200);
}

#[test]
fn backup_skips_usage_and_restore_creates_an_empty_table() {
    let _serial = serial();
    let fixture = Fixture::new();
    let admin = fixture.login(ADMIN);
    let mut web = fixture.service();
    let company = json!({"name":"合成 控え","prefecture":13});
    web.dispatch(
        &request(Some(&admin), "POST", "/api/companies", &company),
        AT + 1,
    );
    web.dispatch(
        &request(Some(&admin), "POST", "/api/companies", &company),
        AT + 61,
    );
    assert_ne!(fixture.saved().len(), 0);
    drop(web);
    let store = fixture.store();
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("snapshot");
    let mut operation = RemoteOperation::begin(&store, "test.backup").unwrap();
    let result = super::remote_backup::create(&store, &mut operation, &destination);
    let snapshot = operation.finish(&store, result).unwrap();
    assert!(
        snapshot
            .manifest()
            .control
            .tables
            .iter()
            .all(|table| table.name != "usage_daily")
    );
    let target = Fixture {
        control: sqld::Sqld::start().unwrap(),
        tenant: sqld::Sqld::start().unwrap(),
    };
    let mut operation = RemoteOperation::begin(&store, "test.restore").unwrap();
    let result =
        super::remote_backup::restore(&store, &mut operation, &destination, target.endpoints());
    operation.finish(&store, result).unwrap();
    target.store().check_ready().unwrap();
    assert_eq!(target.saved().len(), 0);
}
