//! 招待制サーバーのCLI・実HTTP・復旧をつなぐ試験。
use std::fmt::Write as _;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};

const PASSWORD: &str = "synthetic-integration-password";

#[test]
fn public_import_is_limited_to_a_registered_real_tenant() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("service");
    cli(&root, &["init"], false);
    cli(&root, &["tenant-add", "one", "--origin", "real"], false);
    cli(&root, &["tenant-add", "two", "--origin", "real"], false);
    let source = dir.path().join("catalog.db");
    rusqlite::Connection::open(&source).unwrap().execute_batch("CREATE TABLE companies(corporate_number TEXT PRIMARY KEY,legal_name TEXT,prefecture_code TEXT,source_batch_id INTEGER);CREATE TABLE source_batches(id INTEGER PRIMARY KEY,source_type TEXT,state TEXT,signature_verified INTEGER,signature_fingerprint TEXT,as_of TEXT);INSERT INTO source_batches VALUES(1,'nta_monthly_all','complete',1,'fixture','2026-10-23');INSERT INTO companies VALUES('6600000000000','合成の公的fixture','13',1)").unwrap();
    cli(&root, &["sync-nta", "one", source.to_str().unwrap()], false);
    for (slug, expected) in [("one", 1), ("two", 0)] {
        let e = meibo_core::Engine::open(
            &root.join("tenants").join(format!("{slug}.sqlite3")),
            &meibo_core::OpenOptions::default(),
        )
        .unwrap();
        assert_eq!(
            e.count(&meibo_core::types::Filters::default()).unwrap().n,
            expected
        );
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_meibo"))
        .arg("hosted")
        .arg(&root)
        .args(["sync-nta", "../one", source.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

fn cli(root: &Path, args: &[&str], password: bool) {
    let mut process = Command::new(env!("CARGO_BIN_EXE_meibo"))
        .arg("hosted")
        .arg(root)
        .args(args)
        .stdin(if password {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if password {
        process
            .stdin
            .take()
            .unwrap()
            .write_all(PASSWORD.as_bytes())
            .unwrap();
    }
    let result = process.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "hosted CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains(PASSWORD));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(PASSWORD));
}

struct Server {
    child: Child,
    address: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_meibo"))
            .arg("hosted")
            .arg(root)
            .args([
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
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .to_owned();
        Self { child, address }
    }

    fn request(&self, method: &str, path: &str, auth: Option<&Auth>, body: &Value) -> Response {
        let body = if method == "POST" {
            body.to_string()
        } else {
            String::new()
        };
        let mut message = format!(
            "{method} {path} HTTP/1.1\r\nHost: crm.example.test\r\nOrigin: https://crm.example.test\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
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
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(message.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        Response {
            status,
            head: head.into(),
            body: body.into(),
        }
    }

    fn login(&self, tenant: &str, username: &str) -> Auth {
        let response = self.request(
            "POST",
            "/auth/login",
            None,
            &json!({"tenant":tenant,"username":username,"password":PASSWORD}),
        );
        assert_eq!(response.status, 200);
        let cookie = response.header("Set-Cookie");
        for attribute in [
            "__Host-meibo-session=",
            "HttpOnly",
            "Secure",
            "SameSite=Strict",
            "Path=/",
            "Max-Age=28800",
        ] {
            assert!(cookie.contains(attribute));
        }
        assert!(!cookie.contains("Domain="));
        let value: Value = serde_json::from_str(&response.body).unwrap();
        Auth {
            cookie: cookie.split(';').next().unwrap().into(),
            scope: value["storage_scope"].as_str().unwrap().into(),
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
    body: String,
}
impl Response {
    fn header(&self, name: &str) -> &str {
        self.head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .unwrap()
            .1
            .trim()
    }
}

#[test]
fn hosted_cli_http_cookie_roles_isolation_logout_and_recovery_are_connected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("hosted");
    setup(&root);
    {
        let server = Server::start(&root);
        assert_eq!(
            server
                .request("GET", "/api/summary", None, &Value::Null)
                .status,
            401
        );
        let alpha = server.login("alpha", "admin");
        let beta = server.login("beta", "admin");
        assert_summary(&server, &alpha, 2);
        assert_summary(&server, &beta, 4);
        let switched = Auth {
            cookie: beta.cookie.clone(),
            scope: alpha.scope.clone(),
        };
        assert_eq!(
            server
                .request("GET", "/api/summary", Some(&switched), &Value::Null)
                .status,
            409
        );
        assert_eq!(
            server
                .request(
                    "POST",
                    "/api/companies",
                    Some(&beta),
                    &json!({"name":"forbidden","prefecture":13})
                )
                .status,
            403
        );
        assert_eq!(
            server
                .request("GET", "/auth/audit", Some(&beta), &Value::Null)
                .status,
            403
        );
        let editor = server.login("beta", "editor");
        assert_eq!(
            server
                .request(
                    "POST",
                    "/api/companies",
                    Some(&editor),
                    &json!({"name":"合成の追加企業","prefecture":13})
                )
                .status,
            201
        );
        assert_summary(&server, &beta, 5);
        assert_summary(&server, &alpha, 2);
        assert_eq!(
            server
                .request(
                    "POST",
                    "/api/companies/1/contacts/1/erase",
                    Some(&editor),
                    &json!({})
                )
                .status,
            403
        );
        let response = server.request("POST", "/auth/logout", Some(&alpha), &json!({}));
        assert_eq!(response.status, 200);
        assert!(response.header("Set-Cookie").contains("Max-Age=0"));
        assert_eq!(
            server
                .request("GET", "/api/summary", Some(&alpha), &Value::Null)
                .status,
            401
        );
        cli(&root, &["user-disable", "editor"], false);
        assert_eq!(
            server
                .request("GET", "/api/summary", Some(&editor), &Value::Null)
                .status,
            401
        );
    }
    let snapshot = temp.path().join("snapshot");
    cli(&root, &["backup", snapshot.to_str().unwrap()], false);
    let restored = temp.path().join("restored");
    cli(&restored, &["restore", snapshot.to_str().unwrap()], false);
    let server = Server::start(&restored);
    let beta = server.login("beta", "admin");
    let response = server.request("GET", "/api/summary", Some(&beta), &Value::Null);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).unwrap()["total"],
        5
    );
}

fn setup(root: &Path) {
    cli(root, &["init"], false);
    for (tenant, rows) in [("alpha", "2"), ("beta", "4")] {
        cli(
            root,
            &[
                "tenant-add",
                tenant,
                "--origin",
                "synthetic",
                "--demo-rows",
                rows,
            ],
            false,
        );
    }
    for user in ["admin", "editor"] {
        cli(root, &["user-add", user, "--password-stdin"], true);
    }
    cli(root, &["member-set", "admin", "alpha", "admin"], false);
    cli(root, &["member-set", "admin", "beta", "viewer"], false);
    cli(root, &["member-set", "editor", "beta", "editor"], false);
}

fn assert_summary(server: &Server, auth: &Auth, expected: u64) {
    let response = server.request("GET", "/api/summary", Some(auth), &Value::Null);
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(value["total"].as_u64().unwrap(), expected);
}
