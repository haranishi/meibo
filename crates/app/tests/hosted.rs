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

fn cli(root: &Path, args: &[&str], password: bool) -> String {
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
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(!stdout.contains(PASSWORD));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(PASSWORD));
    stdout
}

fn backup(root: &Path, snapshot: &Path) -> String {
    use sha2::{Digest, Sha256};
    let output = cli(root, &["backup", snapshot.to_str().unwrap()], false);
    let bytes = std::fs::read(snapshot.join("manifest.json")).unwrap();
    let fingerprint = format!("{:x}", Sha256::digest(bytes));
    assert_eq!(output, format!("backed-up manifest-sha256={fingerprint}\n"));
    fingerprint
}

fn restore(root: &Path, snapshot: &Path, fingerprint: &str) {
    let args = [
        "restore",
        snapshot.to_str().unwrap(),
        "--manifest-sha256",
        fingerprint,
    ];
    assert_eq!(cli(root, &args, false), "restored\n");
}

#[test]
fn restore_cli_requires_the_saved_manifest_fingerprint() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("service");
    cli(&root, &["init"], false);
    let snapshot = temp.path().join("snapshot");
    let fingerprint = backup(&root, &snapshot);
    let restored = temp.path().join("restored");
    let wrong = "0".repeat(64);
    let non_hex = "g".repeat(64);
    for args in [
        &[][..],
        &["--manifest-sha256", "1234"],
        &["--manifest-sha256", non_hex.as_str()],
        &["--manifest-sha256", wrong.as_str()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_meibo"))
            .arg("hosted")
            .arg(&restored)
            .arg("restore")
            .arg(&snapshot)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!restored.exists());
    }
    restore(&restored, &snapshot, &fingerprint.to_ascii_uppercase());
    assert!(cli(&restored, &["audit"], false).contains("restore"));
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
    let fingerprint = backup(&root, &snapshot);
    let restored = temp.path().join("restored");
    restore(&restored, &snapshot, &fingerprint);
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

fn assert_public_demo_seeded(root: &Path) {
    let engine = meibo_core::Engine::open(
        &root.join("tenants/demo.sqlite3"),
        &meibo_core::OpenOptions::default(),
    )
    .unwrap();
    assert_eq!(engine.origin(), meibo_core::types::DataOrigin::Synthetic);
    assert_eq!(
        engine
            .count(&meibo_core::types::Filters {
                include_excluded: true,
                ..meibo_core::types::Filters::default()
            })
            .unwrap()
            .n,
        150
    );
    let deals = engine
        .crm_opportunities(&meibo_core::crm::OpportunityQuery {
            company: None,
            after: None,
            limit: 50,
        })
        .unwrap();
    assert_eq!(deals.total, 9);
    let month = deals
        .items
        .iter()
        .find(|deal| deal.input.name == "合成・Webサイト制作")
        .and_then(|deal| deal.outcome_date.as_deref())
        .map(|date| &date[..7])
        .unwrap();
    let report = engine.crm_sales_report(month).unwrap();
    assert_eq!(report.won_amount_jpy, 320_000);
    assert_eq!(report.win_rate_percent(), Some(50.0));
    let weighted: u128 = report
        .projections
        .values()
        .map(|p| p.weighted_amount_jpy)
        .sum();
    assert_eq!(weighted, 650_000);
    assert_eq!(report.won_amount_jpy + weighted, 970_000);

    let control = rusqlite::Connection::open(root.join("control.sqlite3")).unwrap();
    let users: Vec<(i64, String, String)> = control
        .prepare("SELECT id, username, password_hash FROM users ORDER BY username")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].1, "demo");
    assert_eq!(users[1].1, "demo-b");
    assert!(
        users
            .iter()
            .all(|(_, _, hash)| hash.starts_with("$argon2id$"))
    );
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    let due = engine.crm_due_followups(Some(users[0].0), now).unwrap();
    assert_eq!(due.total, 1);
}

#[test]
fn public_demo_bootstraps_before_listening_and_never_leaks_passwords_to_output() {
    let temp = tempfile::tempdir().unwrap();
    for (name, create_empty) in [("missing-root", false), ("empty-root", true)] {
        let root = temp.path().join(name);
        if create_empty {
            std::fs::create_dir(&root).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_meibo"))
            .arg("hosted")
            .arg(&root)
            .args([
                "serve",
                "--origin",
                "http://127.0.0.1:4317",
                "--bind",
                "127.0.0.1:0",
                "--allow-http-loopback",
                "--public-demo",
                "--demo-contact",
                "https://example.invalid/issues",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout.read_line(&mut first).unwrap();
        assert!(
            first.starts_with("hosted listening: 127.0.0.1:")
                && first.contains("(public origin: http://127.0.0.1:4317)"),
            "unexpected stdout line for {name}: {first:?}"
        );
        // 待ち受け行が出た時点で初期データがすべて揃っていることを確かめる。
        assert_public_demo_seeded(&root);

        let _ = child.kill();
        let _ = child.wait();
        let mut rest_stdout = String::new();
        stdout.read_to_string(&mut rest_stdout).unwrap();
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(
            rest_stdout.is_empty(),
            "unexpected extra stdout: {rest_stdout:?}"
        );
        assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
    }
}

#[test]
fn public_demo_cli_rejects_populated_root_incompatible_flags_and_preserves_normal_mode() {
    let temp = tempfile::tempdir().unwrap();

    // 既存のファイルがあるROOTを拒否する。
    let populated = temp.path().join("populated");
    std::fs::create_dir(&populated).unwrap();
    std::fs::write(populated.join("real-data.txt"), b"keep").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meibo"))
        .arg("hosted")
        .arg(&populated)
        .args([
            "serve",
            "--origin",
            "https://demo.example.invalid",
            "--bind",
            "127.0.0.1:0",
            "--public-demo",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("既存のデータ"));
    assert_eq!(
        std::fs::read(populated.join("real-data.txt")).unwrap(),
        b"keep"
    );

    // 初期化済みの複数組織・実データROOTを拒否する。
    let existing_hosted = temp.path().join("existing-hosted");
    cli(&existing_hosted, &["init"], false);
    cli(
        &existing_hosted,
        &["tenant-add", "real-a", "--origin", "real"],
        false,
    );
    cli(
        &existing_hosted,
        &["tenant-add", "synth-b", "--origin", "synthetic"],
        false,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_meibo"))
        .arg("hosted")
        .arg(&existing_hosted)
        .args([
            "serve",
            "--origin",
            "https://demo.example.invalid",
            "--bind",
            "127.0.0.1:0",
            "--public-demo",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("既存のデータ"));

    // --access-config併用・不正な--demo-contact・--public-demoなしの--demo-contactを拒否する。
    let untouched = temp.path().join("untouched");
    for args in [
        vec![
            "serve",
            "--origin",
            "https://demo.example.invalid",
            "--public-demo",
            "--access-config",
            "access.json",
        ],
        vec![
            "serve",
            "--origin",
            "https://demo.example.invalid",
            "--public-demo",
            "--demo-contact",
            "javascript:alert(1)",
        ],
        vec![
            "serve",
            "--origin",
            "https://demo.example.invalid",
            "--demo-contact",
            "https://example.invalid/contact",
        ],
        // 通常モードは未初期化ROOTを自動作成しない。
        vec![
            "serve",
            "--origin",
            "https://crm.example.invalid",
            "--bind",
            "127.0.0.1:0",
        ],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_meibo"))
            .arg("hosted")
            .arg(&untouched)
            .args(&args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "should reject args: {args:?}");
        assert!(!untouched.exists(), "should not create root for: {args:?}");
    }
}

struct DemoServer {
    child: Child,
    address: String,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Drop for DemoServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl DemoServer {
    fn start(root: &Path, extra_args: &[&str]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_meibo"));
        command.arg("hosted").arg(root).args([
            "serve",
            "--origin",
            "http://127.0.0.1:4317",
            "--bind",
            "127.0.0.1:0",
            "--allow-http-loopback",
            "--public-demo",
        ]);
        command.args(extra_args);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut first = String::new();
        stdout.read_line(&mut first).unwrap();
        let address = first
            .strip_prefix("hosted listening: ")
            .unwrap_or_else(|| panic!("unexpected listening line: {first:?}"))
            .split(' ')
            .next()
            .unwrap()
            .to_owned();
        Self {
            child,
            address,
            stdout,
        }
    }

    fn send_raw(&self, raw: &str) -> std::io::Result<Response> {
        let mut stream = TcpStream::connect(&self.address)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(raw.as_bytes())?;
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        let boundary = loop {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break None;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(pos) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break Some(pos);
            }
        }
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "malformed response: {:?}",
                String::from_utf8_lossy(&bytes)
            ))
        })?;
        let head = String::from_utf8(bytes[..boundary].to_vec())
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| std::io::Error::other(format!("missing status: {head:?}")))?;
        let content_length = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case("Content-Length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok());
        let mut body = bytes[boundary + 4..].to_vec();
        if let Some(expected) = content_length {
            while body.len() < expected {
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
            body.truncate(expected);
        } else {
            stream.read_to_end(&mut body)?;
        }
        let body =
            String::from_utf8(body).map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Response { status, head, body })
    }

    fn get(&self, path: &str) -> std::io::Result<Response> {
        self.send_raw(&format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:4317\r\nConnection: close\r\n\r\n"
        ))
    }

    fn wait_exit(&mut self, timeout: Duration) -> (std::process::ExitStatus, String, Duration) {
        let start = std::time::Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                start.elapsed() < timeout,
                "demo server did not exit within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let elapsed = start.elapsed();
        let mut out = String::new();
        self.stdout.read_to_string(&mut out).unwrap();
        (status, out, elapsed)
    }
}

#[test]
fn public_demo_exits_on_idle_without_requests_or_with_health_only_and_stays_alive_on_login() {
    let temp = tempfile::tempdir().unwrap();

    // 1. 要求なしなら約2〜4秒以内に終了コード0・demo-reset reason=idle。
    {
        let mut server = DemoServer::start(
            &temp.path().join("idle-none"),
            &["--demo-idle-seconds", "2"],
        );
        let (status, out, elapsed) = server.wait_exit(Duration::from_secs(5));
        assert_eq!(status.code(), Some(0));
        assert_eq!(out.trim(), "demo-reset reason=idle");
        assert!(
            elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_secs(5),
            "unexpected idle elapsed: {elapsed:?}"
        );
    }

    // 2. /health を0.5秒ごとに送り続けてもアイドルの時計は進まず終了する。
    {
        let mut server = DemoServer::start(
            &temp.path().join("idle-health"),
            &["--demo-idle-seconds", "2"],
        );
        let start = std::time::Instant::now();
        let mut health_ok = 0_u32;
        let status = loop {
            if let Some(status) = server.child.try_wait().unwrap() {
                break status;
            }
            if let Ok(reply) = server.get("/health") {
                assert_eq!(reply.status, 200);
                health_ok += 1;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "demo server stayed alive despite only /health traffic"
            );
            std::thread::sleep(Duration::from_millis(500));
        };
        let elapsed = start.elapsed();
        let mut out = String::new();
        server.stdout.read_to_string(&mut out).unwrap();
        assert!(
            health_ok >= 2,
            "expected multiple /health replies, got {health_ok}"
        );
        assert_eq!(status.code(), Some(0));
        assert_eq!(out.trim(), "demo-reset reason=idle");
        assert!(
            elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_secs(5),
            "unexpected /health idle elapsed: {elapsed:?}"
        );
    }

    // 3. GET /login を送り続ける間は終了せず、止めるとアイドルで終了する。
    {
        let mut server = DemoServer::start(
            &temp.path().join("idle-login"),
            &["--demo-idle-seconds", "2"],
        );
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(500));
            let reply = server.get("/login").unwrap();
            assert_eq!(reply.status, 200);
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "demo server exited while receiving /login requests"
            );
        }
        let (status, out, elapsed) = server.wait_exit(Duration::from_secs(5));
        assert_eq!(status.code(), Some(0));
        assert_eq!(out.trim(), "demo-reset reason=idle");
        assert!(
            elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_secs(5),
            "unexpected post-login idle elapsed: {elapsed:?}"
        );
    }
}

#[test]
fn public_demo_exits_on_max_uptime_even_under_continuous_requests() {
    let temp = tempfile::tempdir().unwrap();
    let mut server = DemoServer::start(
        &temp.path().join("max-uptime"),
        &["--demo-max-uptime-seconds", "2"],
    );
    let start = std::time::Instant::now();
    let mut login_ok = 0_u32;
    let status = loop {
        if let Some(status) = server.child.try_wait().unwrap() {
            break status;
        }
        if let Ok(reply) = server.get("/login") {
            assert_eq!(reply.status, 200);
            login_ok += 1;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "demo server did not exit on max uptime"
        );
        std::thread::sleep(Duration::from_millis(250));
    };
    let elapsed = start.elapsed();
    let mut out = String::new();
    server.stdout.read_to_string(&mut out).unwrap();
    assert!(
        login_ok >= 3,
        "expected continuous /login traffic, got {login_ok}"
    );
    assert_eq!(status.code(), Some(0));
    assert_eq!(out.trim(), "demo-reset reason=uptime");
    assert!(
        elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_secs(5),
        "unexpected uptime elapsed: {elapsed:?}"
    );
}

#[test]
fn public_demo_rejects_chunked_expect_oversized_and_exits_stalled_on_slow_body() {
    let temp = tempfile::tempdir().unwrap();
    let mut server =
        DemoServer::start(&temp.path().join("stalled"), &["--demo-stall-seconds", "2"]);

    // 実HTTP境界: Content-Length なし（分割転送なし）のPOST -> 411。
    let missing_len = server
        .send_raw(
            "POST /demo/enter HTTP/1.1\r\nHost: 127.0.0.1:4317\r\nOrigin: http://127.0.0.1:4317\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    assert_eq!(missing_len.status, 411);
    assert_eq!(missing_len.header("X-Content-Type-Options"), "nosniff");
    assert_eq!(missing_len.header("Cache-Control"), "no-store");
    assert!(serde_json::from_str::<Value>(&missing_len.body).unwrap()["error"].is_string());

    // 実HTTP境界: Transfer-Encoding: chunked (Content-Length なし) -> 411。
    let chunked = server
        .send_raw(
            "POST /demo/enter HTTP/1.1\r\nHost: 127.0.0.1:4317\r\nOrigin: http://127.0.0.1:4317\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    assert_eq!(chunked.status, 411);
    assert_eq!(chunked.header("X-Content-Type-Options"), "nosniff");
    assert_eq!(chunked.header("Cache-Control"), "no-store");
    assert!(serde_json::from_str::<Value>(&chunked.body).unwrap()["error"].is_string());

    // 実HTTP境界: Expect ヘッダー -> 417（本文を送らなくても即座に拒否）。
    let expect = server
        .send_raw(
            "POST /demo/enter HTTP/1.1\r\nHost: 127.0.0.1:4317\r\nOrigin: http://127.0.0.1:4317\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\nExpect: 100-continue\r\nContent-Length: 2\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    assert_eq!(expect.status, 417);
    assert_eq!(expect.header("X-Content-Type-Options"), "nosniff");
    assert!(serde_json::from_str::<Value>(&expect.body).unwrap()["error"].is_string());

    // 実HTTP境界: 宣言長が上限超過（tiny_http の1024B先行バッファを超える大きさ）-> 413（本文を送らなくても即座に拒否）。
    let oversized = server
        .send_raw(
            "POST /demo/enter HTTP/1.1\r\nHost: 127.0.0.1:4317\r\nOrigin: http://127.0.0.1:4317\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\nContent-Length: 2048\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    assert_eq!(oversized.status, 413);
    assert_eq!(oversized.header("X-Content-Type-Options"), "nosniff");
    assert!(serde_json::from_str::<Value>(&oversized.body).unwrap()["error"].is_string());

    // tiny_http 0.12 は Content-Length <= 1024 を接続スレッド側で先行読込するため、
    // 1025バイト以上かつ64KiB以下の POST で本文を送らずに待たせると、待ち受けループが read_to_end で停止し、
    // 見張りが終了コード70・reason=stalledで終了させる。
    let mut stalled_stream = TcpStream::connect(&server.address).unwrap();
    stalled_stream
        .write_all(
            b"POST /api/companies HTTP/1.1\r\nHost: 127.0.0.1:4317\r\nOrigin: http://127.0.0.1:4317\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n",
        )
        .unwrap();
    let (status, out, elapsed) = server.wait_exit(Duration::from_secs(6));
    drop(stalled_stream);
    assert_eq!(status.code(), Some(70));
    assert_eq!(out.trim(), "demo-reset reason=stalled");
    assert!(
        elapsed >= Duration::from_millis(1500) && elapsed <= Duration::from_secs(6),
        "unexpected stall elapsed: {elapsed:?}"
    );
}

#[test]
fn public_demo_exits_after_storage_stays_at_limit_for_ten_seconds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage-full");
    let mut server = DemoServer::start(&root, &[]);

    let filler = std::fs::File::create(root.join("storage-fill.bin")).unwrap();
    filler.set_len(33 * 1024 * 1024).unwrap();
    drop(filler);

    let (status, out, elapsed) = server.wait_exit(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0));
    assert_eq!(out.trim(), "demo-reset reason=storage");
    assert!(
        elapsed >= Duration::from_secs(9) && elapsed <= Duration::from_secs(20),
        "unexpected storage reset elapsed: {elapsed:?}"
    );
}
