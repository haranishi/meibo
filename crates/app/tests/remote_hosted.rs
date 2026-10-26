//! Remote command and real HTTP contracts on isolated synthetic servers.
#![cfg(feature = "integration-tests")]

use std::{
    fmt::Write as _,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::Mutex,
    time::Duration,
};

use meibo_db::{Connection, RemoteOptions};
use serde_json::{Value, json};

#[allow(dead_code)]
#[path = "../../db/tests/support/client.rs"]
mod client;
#[allow(dead_code)]
#[path = "../../db/tests/support/proxy.rs"]
mod proxy;
#[allow(dead_code)]
#[path = "../../db/tests/support/sqld.rs"]
mod sqld;

const PASSWORD: &str = "synthetic-remote-integration-password";
const TOKEN: &str = "synthetic-remote-integration-token";
static FIXTURES: Mutex<()> = Mutex::new(());

struct Fixture {
    directory: tempfile::TempDir,
    config: PathBuf,
    control: sqld::Sqld,
    alpha: sqld::Sqld,
    beta: sqld::Sqld,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("remote.json");
        std::fs::write(&config, serde_json::to_vec(&json!({"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"},"beta":{"url_env":"MEIBO_DB_BETA_URL","token_env":"MEIBO_DB_BETA_TOKEN"}}})).unwrap()).unwrap();
        Self {
            directory,
            config,
            control: sqld::Sqld::start().unwrap(),
            alpha: sqld::Sqld::start().unwrap(),
            beta: sqld::Sqld::start().unwrap(),
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_meibo"));
        command
            .arg("hosted-remote")
            .arg(&self.config)
            .args(args)
            .env("MEIBO_TEST_REMOTE_LOOPBACK", "1")
            .env("MEIBO_DB_CONTROL_URL", self.control.endpoint())
            .env("MEIBO_DB_CONTROL_TOKEN", TOKEN)
            .env("MEIBO_DB_ALPHA_URL", self.alpha.endpoint())
            .env("MEIBO_DB_ALPHA_TOKEN", TOKEN)
            .env("MEIBO_DB_BETA_URL", self.beta.endpoint())
            .env("MEIBO_DB_BETA_TOKEN", TOKEN);
        command
    }
    fn output(&self, args: &[&str], password: Option<&str>) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(password) = password {
            writeln!(child.stdin.take().unwrap(), "{password}").unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }
    fn cli(&self, args: &[&str], password: Option<&str>) -> String {
        let output = self.output(args, password);
        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains(PASSWORD) && !stdout.contains(TOKEN));
        assert!(!stdout.contains(&self.control.endpoint()));
        stdout
    }
    fn connection(&self) -> Connection {
        Connection::open_remote(
            RemoteOptions::new(self.control.endpoint(), TOKEN).allow_loopback_http_for_tests(),
        )
        .unwrap()
    }
    fn setup(&self) {
        self.cli(&["init"], None);
        for (slug, rows) in [("alpha", "2"), ("beta", "4")] {
            self.cli(
                &[
                    "tenant-add",
                    slug,
                    "--origin",
                    "synthetic",
                    "--demo-rows",
                    rows,
                ],
                None,
            );
        }
        for name in ["admin", "editor", "viewer"] {
            self.cli(&["user-add", name, "--password-stdin"], Some(PASSWORD));
        }
        for (name, slug, role) in [
            ("admin", "alpha", "admin"),
            ("admin", "beta", "viewer"),
            ("editor", "beta", "editor"),
            ("viewer", "alpha", "viewer"),
        ] {
            self.cli(&["member-set", name, slug, role], None);
        }
    }
}

struct Web {
    child: Child,
    address: String,
}

#[test]
fn remote_backup_command_creates_a_verified_service_manifest() {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    fixture.setup();
    let destination = fixture.directory.path().join("typed-service-backup");
    let output = fixture.cli(&["backup", destination.to_str().unwrap()], None);
    assert_eq!(output.trim(), "backed-up");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["tenants"].as_array().unwrap().len(), 2);
    assert_eq!(
        fixture
            .connection()
            .query_row("SELECT count(*) FROM remote_gate", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Web {
    fn start(fixture: &Fixture) -> Self {
        let mut child = fixture
            .command(&[
                "serve",
                "--origin",
                "https://crm.example.test",
                "--bind",
                "127.0.0.1:0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut first)
            .unwrap();
        let address = first
            .strip_prefix("hosted listening: ")
            .expect("actual hosted startup")
            .split(' ')
            .next()
            .unwrap()
            .to_owned();
        Self { child, address }
    }
    fn request(&self, method: &str, path: &str, auth: Option<&Auth>, body: &Value) -> Response {
        self.headers(
            method,
            path,
            auth,
            body,
            ("crm.example.test", "https://crm.example.test", true),
        )
    }
    fn headers(
        &self,
        method: &str,
        path: &str,
        auth: Option<&Auth>,
        body: &Value,
        boundary: (&str, &str, bool),
    ) -> Response {
        let (host, origin, csrf) = boundary;
        let body = if method == "POST" {
            body.to_string()
        } else {
            String::new()
        };
        let mut message = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if csrf {
            message.push_str("X-Meibo-Local: 1\r\n");
        }
        if let Some(auth) = auth {
            write!(
                message,
                "Cookie: {}\r\nX-Meibo-Context: {}\r\n",
                auth.cookie, auth.scope
            )
            .unwrap();
        }
        message.push_str("\r\n");
        message.push_str(&body);
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        stream.write_all(message.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        Response {
            status: head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head: head.into(),
            body: serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.into())),
        }
    }
    fn login(&self, tenant: &str, username: &str, password: &str) -> Auth {
        let response = self.request(
            "POST",
            "/auth/login",
            None,
            &json!({"tenant":tenant,"username":username,"password":password}),
        );
        assert_eq!(response.status, 200);
        let cookie = response.header("Set-Cookie");
        for item in [
            "__Host-meibo-session=",
            "Secure",
            "HttpOnly",
            "SameSite=Strict",
            "Path=/",
        ] {
            assert!(cookie.contains(item));
        }
        Auth {
            cookie: cookie.split(';').next().unwrap().into(),
            scope: response.body["storage_scope"].as_str().unwrap().into(),
        }
    }
}
struct Auth {
    cookie: String,
    scope: String,
}
struct Response {
    status: u16,
    head: String,
    body: Value,
}
impl Response {
    fn header(&self, name: &str) -> &str {
        self.head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .unwrap()
            .1
            .trim()
    }
}
fn total(web: &Web, auth: &Auth, expected: u64) {
    let response = web.request("GET", "/api/summary", Some(auth), &Value::Null);
    assert_eq!(response.status, 200);
    assert_eq!(response.body["total"], expected);
}

#[test]
fn remote_cli_seeds_before_registration_and_manages_stdin_credentials() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    fixture.cli(&["init"], None);
    assert!(
        !fixture
            .output(
                &["tenant-add", "beta", "--origin", "real", "--demo-rows", "1"],
                None
            )
            .status
            .success()
    );
    fixture.cli(
        &[
            "tenant-add",
            "alpha",
            "--origin",
            "synthetic",
            "--demo-rows",
            "3",
        ],
        None,
    );
    fixture.cli(
        &[
            "tenant-add",
            "beta",
            "--origin",
            "synthetic",
            "--demo-rows",
            "4",
        ],
        None,
    );
    fixture.cli(&["user-add", "admin", "--password-stdin"], Some(PASSWORD));
    fixture.cli(&["member-set", "admin", "alpha", "admin"], None);
    assert!(
        !fixture
            .output(&["user-add", "admin", "--password-stdin"], Some(PASSWORD))
            .status
            .success()
    );
    assert!(
        !fixture
            .output(&["user-add", "argv-password", PASSWORD], None)
            .status
            .success()
    );
    let conn = fixture.connection();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM remote_tenants", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM remote_gate", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let web = Web::start(&fixture);
    let auth = web.login("alpha", "admin", PASSWORD);
    total(&web, &auth, 3);
    fixture.cli(
        &["password-set", "admin", "--password-stdin"],
        Some("synthetic-replacement-password"),
    );
    assert_eq!(
        web.request("GET", "/api/summary", Some(&auth), &Value::Null)
            .status,
        401
    );
    let auth = web.login("alpha", "admin", "synthetic-replacement-password");
    fixture.cli(&["revoke", "admin"], None);
    assert_eq!(
        web.request("GET", "/api/summary", Some(&auth), &Value::Null)
            .status,
        401
    );
    let auth = web.login("alpha", "admin", "synthetic-replacement-password");
    fixture.cli(&["user-disable", "admin"], None);
    assert_eq!(
        web.request("GET", "/api/summary", Some(&auth), &Value::Null)
            .status,
        401
    );
    assert_ne!(fixture.cli(&["audit", "alpha"], None), "");
}

#[test]
fn remote_cli_http_share_authentication_roles_context_and_gate() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    fixture.setup();
    let web = Web::start(&fixture);
    assert_eq!(
        web.request("GET", "/api/summary", None, &Value::Null)
            .status,
        401
    );
    let alpha = web.login("alpha", "admin", PASSWORD);
    let beta = web.login("beta", "admin", PASSWORD);
    let editor = web.login("beta", "editor", PASSWORD);
    total(&web, &alpha, 2);
    total(&web, &beta, 4);
    assert_eq!(
        web.request(
            "POST",
            "/api/companies",
            Some(&beta),
            &json!({"name":"拒否企業","prefecture":13})
        )
        .status,
        403
    );
    assert_eq!(
        web.request("GET", "/auth/audit", Some(&editor), &Value::Null)
            .status,
        403
    );
    assert_eq!(
        web.request("GET", "/auth/audit", Some(&alpha), &Value::Null)
            .status,
        200
    );
    assert_eq!(
        web.request(
            "POST",
            "/api/companies",
            Some(&editor),
            &json!({"name":"合成の追加企業","prefecture":13})
        )
        .status,
        201
    );
    total(&web, &beta, 5);
    total(&web, &alpha, 2);
    let export = web.request("GET", "/api/export", Some(&beta), &Value::Null);
    assert_eq!(export.status, 200);
    assert!(export.header("Content-Type").starts_with("text/csv"));
    assert!(
        web.request("GET", "/", Some(&alpha), &Value::Null)
            .body
            .as_str()
            .unwrap()
            .contains("data-meibo-gated")
    );
    let switched = Auth {
        cookie: beta.cookie.clone(),
        scope: alpha.scope.clone(),
    };
    assert_eq!(
        web.request("GET", "/api/summary", Some(&switched), &Value::Null)
            .status,
        409
    );
    assert_boundary(&web, &alpha, &editor);
    assert_eq!(
        web.request("POST", "/auth/logout", Some(&alpha), &json!({}))
            .status,
        200
    );
    assert_eq!(
        web.request("GET", "/api/summary", Some(&alpha), &Value::Null)
            .status,
        401
    );
    assert_gate_recovery(&fixture, &web);
}

fn assert_boundary(web: &Web, alpha: &Auth, editor: &Auth) {
    assert_eq!(
        web.headers(
            "GET",
            "/api/summary",
            Some(alpha),
            &Value::Null,
            ("foreign.example.test", "https://crm.example.test", true),
        )
        .status,
        403
    );
    assert_eq!(
        web.headers(
            "POST",
            "/api/companies",
            Some(editor),
            &json!({}),
            ("crm.example.test", "https://foreign.example.test", true),
        )
        .status,
        403
    );
    assert_eq!(
        web.headers(
            "POST",
            "/api/companies",
            Some(editor),
            &json!({}),
            ("crm.example.test", "https://crm.example.test", false),
        )
        .status,
        403
    );
}

fn assert_gate_recovery(fixture: &Fixture, web: &Web) {
    let conn = fixture.connection();
    conn.execute(
        "INSERT INTO remote_gate VALUES(1,?1,0,'interrupted.fixture')",
        [vec![8_u8; 32]],
    )
    .unwrap();
    assert_eq!(
        web.request("GET", "/health", None, &Value::Null).status,
        503
    );
    assert!(
        !fixture
            .output(&["user-add", "blocked", "--password-stdin"], Some(PASSWORD))
            .status
            .success()
    );
    let status: Value = serde_json::from_str(&fixture.cli(&["gate-status"], None)).unwrap();
    let owner = status["owner"].as_str().unwrap();
    assert!(
        !fixture
            .output(&["gate-recover", owner], None)
            .status
            .success()
    );
    assert!(
        !fixture
            .output(
                &["gate-recover", &"0".repeat(64), "--prior-work-terminated"],
                None
            )
            .status
            .success()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fixture.cli(&["gate-status"], None)).unwrap()["owner"],
        owner
    );
    fixture.cli(&["gate-recover", owner, "--prior-work-terminated"], None);
    assert_eq!(
        web.request("GET", "/health", None, &Value::Null).status,
        200
    );
}

#[test]
fn failed_seed_stays_unregistered_and_cli_reports_uncertainty_without_resend() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    fixture.cli(&["init"], None);
    let proxy = proxy::FaultProxy::start(fixture.alpha.address()).unwrap();
    proxy.arm(proxy::Fault::LoseMatchingWriteResponse(
        "INSERT INTO companies",
    ));
    let output = fixture
        .command(&[
            "tenant-add",
            "alpha",
            "--origin",
            "synthetic",
            "--demo-rows",
            "2",
        ])
        .env("MEIBO_DB_ALPHA_URL", proxy.endpoint())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("outcome_unknown"));
    assert!(!stderr.contains(TOKEN));
    assert!(!stderr.contains(&proxy.endpoint()));
    assert_eq!(proxy.triggered(), 1);
    let sent = proxy.requests();
    let conn = fixture.connection();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM tenants", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM remote_tenants", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM remote_gate", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let status: Value = serde_json::from_str(&fixture.cli(&["gate-status"], None)).unwrap();
    fixture.cli(
        &[
            "gate-recover",
            status["owner"].as_str().unwrap(),
            "--prior-work-terminated",
        ],
        None,
    );
    assert_eq!(proxy.requests(), sent);
    fixture.cli(
        &["user-add", "after-recovery", "--password-stdin"],
        Some(PASSWORD),
    );
}

#[test]
fn remote_sync_nta_reads_only_the_synthetic_fixture_catalog_and_selected_tenant() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    fixture.cli(&["init"], None);
    fixture.cli(&["tenant-add", "alpha", "--origin", "real"], None);
    fixture.cli(&["tenant-add", "beta", "--origin", "real"], None);
    let source = fixture.directory.path().join("catalog.db");
    let catalog = rusqlite::Connection::open(&source).unwrap();
    catalog.execute_batch("CREATE TABLE companies(corporate_number TEXT PRIMARY KEY,legal_name TEXT,prefecture_code TEXT,source_batch_id INTEGER);CREATE TABLE source_batches(id INTEGER PRIMARY KEY,source_type TEXT,state TEXT,signature_verified INTEGER,signature_fingerprint TEXT,as_of TEXT);INSERT INTO source_batches VALUES(1,'nta_monthly_all','complete',1,'fixture','2026-10-23');INSERT INTO companies VALUES('6600000000000','合成の公的fixture','13',1)").unwrap();
    let report = fixture.cli(
        &[
            "sync-nta",
            "alpha",
            source.to_str().unwrap(),
            "--limit",
            "1",
        ],
        None,
    );
    assert!(report.contains("inserted=1"));
    for (server, expected) in [(&fixture.alpha, 1_i64), (&fixture.beta, 0)] {
        let conn = Connection::open_remote(
            RemoteOptions::new(server.endpoint(), TOKEN).allow_loopback_http_for_tests(),
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM companies", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        catalog
            .query_row("SELECT count(*) FROM companies", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        !fixture
            .output(&["sync-nta", "../alpha", source.to_str().unwrap()], None)
            .status
            .success()
    );
    assert_eq!(
        fixture
            .connection()
            .query_row("SELECT count(*) FROM remote_gate", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn loopback_database_policy_is_explicit_and_help_has_only_implemented_commands() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let output = fixture
        .command(&["init"])
        .env_remove("MEIBO_TEST_REMOTE_LOOPBACK")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("リモート保存設定")
    );
    assert_eq!(
        fixture
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let help = fixture.cli(&["--help"], None);
    for word in [
        "gate-status",
        "gate-recover",
        "tenant-add",
        "sync-nta",
        "backup NEW_DIRECTORY",
    ] {
        assert!(help.contains(word));
    }
    assert!(!help.contains("restore") && !help.contains("MEIBO_TEST_REMOTE_LOOPBACK"));
}
