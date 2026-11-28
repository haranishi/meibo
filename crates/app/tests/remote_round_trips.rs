//! Count database round trips for the main remote HTTP requests.
//!
//! This file depends only on the public CLI/HTTP behavior and the shared
//! loopback fixtures, so the same file can measure an older revision. Set
//! `MEIBO_ROUND_TRIP_LIMITS=off` to print counts without the upper bounds.
#![cfg(feature = "integration-tests")]

use std::{
    fmt::Write as _,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    time::Duration,
};

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

const PASSWORD: &str = "synthetic-round-trip-password";
const TOKEN: &str = "synthetic-round-trip-token";
const HOST: &str = "crm.example.test";
const ORIGIN: &str = "https://crm.example.test";

struct Servers {
    directory: tempfile::TempDir,
    control: sqld::Sqld,
    alpha: sqld::Sqld,
}

impl Servers {
    fn command(&self, control: &str, alpha: &str, args: &[&str]) -> Command {
        let config = self.directory.path().join("remote.json");
        let mut command = Command::new(env!("CARGO_BIN_EXE_meibo"));
        command
            .arg("hosted-remote")
            .arg(config)
            .args(args)
            .env("MEIBO_TEST_REMOTE_LOOPBACK", "1")
            .env("MEIBO_DB_CONTROL_URL", control)
            .env("MEIBO_DB_CONTROL_TOKEN", TOKEN)
            .env("MEIBO_DB_ALPHA_URL", alpha)
            .env("MEIBO_DB_ALPHA_TOKEN", TOKEN);
        command
    }

    fn cli(&self, args: &[&str], password: Option<&str>) {
        let mut child = self
            .command(&self.control.endpoint(), &self.alpha.endpoint(), args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(password) = password {
            writeln!(stdin, "{password}").unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "setup command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

struct Web {
    child: Child,
    address: String,
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Reply {
    status: u16,
    head: String,
    body: Value,
}

impl Web {
    fn start(servers: &Servers, control: &str, alpha: &str) -> Self {
        let mut child = servers
            .command(
                control,
                alpha,
                &["serve", "--origin", ORIGIN, "--bind", "127.0.0.1:0"],
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut first = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut first)
            .unwrap();
        let address = first
            .strip_prefix("hosted listening: ")
            .expect("hosted startup line")
            .split(' ')
            .next()
            .unwrap()
            .to_owned();
        Self { child, address }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        auth: Option<&(String, String)>,
        body: &Value,
    ) -> Reply {
        let body = if method == "POST" {
            body.to_string()
        } else {
            String::new()
        };
        let mut message = format!(
            "{method} {path} HTTP/1.1\r\nHost: {HOST}\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\nX-Meibo-Local: 1\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some((cookie, scope)) = auth {
            write!(message, "Cookie: {cookie}\r\nX-Meibo-Context: {scope}\r\n").unwrap();
        }
        message.push_str("\r\n");
        message.push_str(&body);
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream.write_all(message.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        Reply {
            status: head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head: head.to_owned(),
            body: serde_json::from_str(body).unwrap_or(Value::Null),
        }
    }
}

struct Counter<'a> {
    control: &'a proxy::FaultProxy,
    tenant: &'a proxy::FaultProxy,
    rows: Vec<(String, usize, usize)>,
}

impl Counter<'_> {
    fn measure<T>(&mut self, name: &str, run: impl FnOnce() -> T) -> T {
        let (control, tenant) = (self.control.requests(), self.tenant.requests());
        let value = run();
        // The hosted reply is sent after its last database response returns.
        let control = self.control.requests() - control;
        let tenant = self.tenant.requests() - tenant;
        println!(
            "round-trips {name}: control={control} tenant={tenant} total={}",
            control + tenant
        );
        self.rows.push((name.to_owned(), control, tenant));
        value
    }
}

fn limit(rows: &[(String, usize, usize)], name: &str, maximum: usize) {
    let (_, control, tenant) = rows.iter().find(|row| row.0 == name).unwrap();
    assert!(
        control + tenant <= maximum,
        "{name}: {} round trips exceeds {maximum}",
        control + tenant
    );
}

impl Servers {
    fn start() -> Self {
        let servers = Self {
            directory: tempfile::tempdir().unwrap(),
            control: sqld::Sqld::start().unwrap(),
            alpha: sqld::Sqld::start().unwrap(),
        };
        std::fs::write(
            servers.directory.path().join("remote.json"),
            serde_json::to_vec(&json!({"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"}}})).unwrap(),
        )
        .unwrap();
        servers.cli(&["init"], None);
        servers.cli(
            &[
                "tenant-add",
                "alpha",
                "--origin",
                "synthetic",
                "--demo-rows",
                "5",
            ],
            None,
        );
        servers.cli(&["user-add", "admin", "--password-stdin"], Some(PASSWORD));
        servers.cli(&["member-set", "admin", "alpha", "admin"], None);
        servers
    }
}

#[test]
fn main_remote_requests_report_their_database_round_trips() {
    let servers = Servers::start();
    let control = proxy::FaultProxy::start(servers.control.address()).unwrap();
    let tenant = proxy::FaultProxy::start(servers.alpha.address()).unwrap();
    let web = Web::start(&servers, &control.endpoint(), &tenant.endpoint());
    let mut counter = Counter {
        control: &control,
        tenant: &tenant,
        rows: Vec::new(),
    };

    let login = counter.measure("POST /auth/login", || {
        web.request(
            "POST",
            "/auth/login",
            None,
            &json!({"tenant":"alpha","username":"admin","password":PASSWORD}),
        )
    });
    assert_eq!(login.status, 200);
    let scope = login.body["storage_scope"].as_str().unwrap().to_owned();
    let cookie = login
        .head
        .lines()
        .find_map(|line| line.strip_prefix("Set-Cookie: "))
        .and_then(|cookie| cookie.split(';').next())
        .unwrap()
        .to_owned();
    let auth = Some((cookie, scope));
    let auth = auth.as_ref();

    for (name, target) in [
        ("GET / (signed in)", "/"),
        ("GET /api/summary", "/api/summary"),
    ] {
        let reply = counter.measure(name, || web.request("GET", target, auth, &Value::Null));
        assert_eq!(reply.status, 200, "{name}");
    }
    let list = counter.measure("GET /api/companies", || {
        web.request("GET", "/api/companies?limit=50", auth, &Value::Null)
    });
    assert_eq!(list.status, 200);
    let id = list.body["rows"][0]["id"].as_i64().unwrap();
    let detail = format!("/api/companies/{id}");
    let activity = format!("/api/companies/{id}/activities");
    for (name, method, target, signed_in, body) in [
        (
            "GET /api/companies?q= (search)",
            "GET",
            "/api/companies?q=%E5%90%88%E6%88%90",
            true,
            Value::Null,
        ),
        (
            "GET /api/companies/{id}",
            "GET",
            detail.as_str(),
            true,
            Value::Null,
        ),
        (
            "GET /api/work-items",
            "GET",
            "/api/work-items",
            true,
            Value::Null,
        ),
        (
            "GET /api/opportunities",
            "GET",
            "/api/opportunities",
            true,
            Value::Null,
        ),
        (
            "POST /api/companies/{id}/activities",
            "POST",
            activity.as_str(),
            true,
            json!({"note":"合成の架電結果"}),
        ),
        (
            "GET /api/summary (again)",
            "GET",
            "/api/summary",
            true,
            Value::Null,
        ),
        ("GET /login", "GET", "/login", false, Value::Null),
        ("GET /health", "GET", "/health", false, Value::Null),
    ] {
        let auth = if signed_in { auth } else { None };
        let reply = counter.measure(name, || web.request(method, target, auth, &body));
        assert_eq!(reply.status, 200, "{name}");
        if target == "/api/summary" {
            assert_eq!(reply.body["total"], 5);
        }
    }

    if std::env::var("MEIBO_ROUND_TRIP_LIMITS").as_deref() != Ok("off") {
        limits(&counter.rows);
    }
}

/// Upper bounds after remote-performance stages 1 and 2 (design target: the
/// summary in five round trips or fewer on the gate-free path). The first
/// signed-in request also opens the tenant database.
fn limits(rows: &[(String, usize, usize)]) {
    for (name, maximum) in [
        ("POST /auth/login", 40),
        ("GET / (signed in)", 22),
        ("GET /api/summary", 5),
        ("GET /api/summary (again)", 5),
        ("GET /api/companies", 10),
        ("GET /api/companies/{id}", 14),
        ("GET /api/work-items", 12),
        ("GET /api/opportunities", 14),
        ("POST /api/companies/{id}/activities", 30),
        ("GET /login", 0),
        ("GET /health", 6),
    ] {
        limit(rows, name, maximum);
    }
}
