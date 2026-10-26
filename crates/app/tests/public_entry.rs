//! Public service metadata and anonymous assets over the actual HTTP server.
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use fs4::fs_std::FileExt;
use meibo_core::{Engine, types::DataOrigin};

const ORIGIN: &str = "https://crm.example.test";
const HOST: &str = "crm.example.test";
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";
const ASSETS: [&str; 2] = ["/public-entry.js", "/public-entry.png"];

struct Running {
    child: Child,
    address: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Running {
    fn start(root: &Path, development: bool) -> Self {
        let result = Command::new(env!("CARGO_BIN_EXE_meibo"))
            .arg("hosted")
            .arg(root)
            .arg("init")
            .output()
            .unwrap();
        assert!(result.status.success());
        let mut command = Command::new(env!("CARGO_BIN_EXE_meibo"));
        command.arg("hosted").arg(root).args([
            "serve",
            "--origin",
            if development {
                "http://localhost:9999"
            } else {
                ORIGIN
            },
            "--bind",
            "127.0.0.1:0",
        ]);
        if development {
            command.arg("--allow-http-loopback");
        }
        Self::spawn(command, "hosted listening: ")
    }

    fn spawn(mut command: Command, prefix: &str) -> Self {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let _ = sender.send(line);
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });
        let mut running = Self {
            child,
            address: String::new(),
        };
        let line = receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("fixture startup");
        running.address = line
            .trim()
            .strip_prefix(prefix)
            .expect("fixture address")
            .split(' ')
            .next()
            .unwrap()
            .trim_end_matches('/')
            .into();
        running
    }

    fn request(&self, method: &str, path: &str, host: &str, extra: &str) -> Response {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: 0\r\n{extra}\r\n").unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let boundary = bytes.windows(4).position(|v| v == b"\r\n\r\n").unwrap();
        let head = String::from_utf8(bytes[..boundary].to_vec()).unwrap();
        let raw = &bytes[boundary + 4..];
        let body = if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            decode_chunks(raw)
        } else {
            raw.to_vec()
        };
        Response {
            status: head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head,
            body,
        }
    }
}

fn decode_chunks(mut raw: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let boundary = raw
            .windows(2)
            .position(|v| v == b"\r\n")
            .expect("chunk size");
        let size = usize::from_str_radix(
            std::str::from_utf8(&raw[..boundary])
                .unwrap()
                .split(';')
                .next()
                .unwrap(),
            16,
        )
        .unwrap();
        if size == 0 {
            break;
        }
        raw = &raw[boundary + 2..];
        body.extend_from_slice(&raw[..size]);
        assert_eq!(&raw[size..size + 2], b"\r\n");
        raw = &raw[size + 2..];
    }
    body
}

struct Response {
    status: u16,
    head: String,
    body: Vec<u8>,
}
impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
    fn secure_public(&self, mime: &str) {
        assert_eq!(self.status, 200);
        assert_eq!(self.header("Content-Type"), Some(mime));
        assert_eq!(self.header("Content-Security-Policy"), Some(CSP));
        assert_eq!(self.header("Referrer-Policy"), Some("no-referrer"));
        assert_eq!(self.header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(self.header("Cache-Control"), Some("no-store"));
        assert_eq!(self.header("Set-Cookie"), None);
    }
}

#[test]
fn public_entry_metadata_and_assets_are_anonymous_fixed_and_same_origin() {
    let directory = tempfile::tempdir().unwrap();
    let server = Running::start(&directory.path().join("hosted"), false);
    let response = server.request("GET", "/login", HOST, "");
    response.secure_public("text/html; charset=utf-8");
    let html = response.text();
    for expected in [
        format!("<link rel=\"canonical\" href=\"{ORIGIN}/login\">"),
        format!("<meta property=\"og:url\" content=\"{ORIGIN}/login\">"),
        format!("<meta property=\"og:image\" content=\"{ORIGIN}/public-entry.png\">"),
        "<meta property=\"og:type\" content=\"website\">".into(),
        "<meta property=\"og:title\" content=\"meibo · 営業ワークスペース\">".into(),
        "<meta property=\"og:description\" content=\"企業情報と営業活動を、チームのワークスペースで。\">".into(),
        "<meta name=\"twitter:card\" content=\"summary_large_image\">".into(),
        "<script src=\"/public-entry.js\" defer></script>".into(),
        "id=\"login-form\"".into(),
        "id=\"public-entry-share\"".into(),
        "aria-live=\"polite\"".into(),
        "rel=\"noopener noreferrer\"".into(),
    ] {
        assert!(html.contains(&expected), "missing public contract: {expected}");
    }
    assert!(html.find("property=\"og:image\"").unwrap() < html.find("<script").unwrap());
    assert!(!html.contains("Instagram") && !html.contains("YouTube"));
    let changed = server.request("GET", "/login?tenant=private-org&username=private-user&customer=private-customer&session=private-session", HOST,
        "Cookie: __Host-meibo-session=private-cookie\r\n");
    assert_eq!(response.body, changed.body);

    let image = server.request("GET", ASSETS[1], HOST, "");
    image.secure_public("image/png");
    assert_eq!(&image.body[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&image.body[12..16], b"IHDR");
    assert_eq!(
        u32::from_be_bytes(image.body[16..20].try_into().unwrap()),
        1200
    );
    assert_eq!(
        u32::from_be_bytes(image.body[20..24].try_into().unwrap()),
        630
    );
    let changed_image = server.request(
        "GET",
        "/public-entry.png?tenant=private-org&customer=private-customer",
        HOST,
        "Cookie: __Host-meibo-session=private-cookie\r\n",
    );
    assert_eq!(image.body, changed_image.body);
    let script = server.request("GET", ASSETS[0], HOST, "");
    script.secure_public("text/javascript; charset=utf-8");
    assert!(script.text().contains("navigator.share"));
}

#[test]
fn public_entry_preserves_host_method_csrf_gate_and_authentication_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("hosted");
    let server = Running::start(&root, false);
    for path in ["/login", ASSETS[0], ASSETS[1]] {
        assert_eq!(
            server.request("GET", path, "evil.example.test", "").status,
            403
        );
        assert_eq!(server.request("PUT", path, HOST, "").status, 405);
        assert_eq!(server.request("POST", path, HOST, "").status, 403);
    }
    assert_eq!(server.request("GET", "/api/summary", HOST, "").status, 401);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".maintenance.lock"))
        .unwrap();
    FileExt::lock_exclusive(&lock).unwrap();
    for path in ["/login", ASSETS[0], ASSETS[1]] {
        assert_eq!(server.request("GET", path, HOST, "").status, 503);
    }
    FileExt::unlock(&lock).unwrap();
    for path in ["/login", ASSETS[0], ASSETS[1]] {
        assert_eq!(server.request("GET", path, HOST, "").status, 200);
    }
}

#[test]
fn development_login_and_standalone_ui_do_not_publish_service_sharing() {
    let directory = tempfile::tempdir().unwrap();
    let development = Running::start(&directory.path().join("hosted"), true);
    let response = development.request("GET", "/login", "localhost:9999", "");
    assert_eq!(response.status, 200);
    assert!(response.text().contains("id=\"login-form\""));
    for public in [
        "rel=\"canonical\"",
        "property=\"og:",
        "/public-entry.js",
        "id=\"public-entry-share\"",
    ] {
        assert!(!response.text().contains(public));
    }
    let database = directory.path().join("synthetic.db");
    drop(
        Engine::create(
            &database,
            DataOrigin::Synthetic,
            &meibo_core::OpenOptions::default(),
        )
        .unwrap(),
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_meibo"));
    command.arg("serve").arg(database).args(["--port", "0"]);
    let local = Running::spawn(command, "http://");
    let response = local.request("GET", "/", &local.address, "");
    assert_eq!(response.status, 200);
    assert!(!response.text().contains("public-entry"));
    assert!(!response.text().contains("property=\"og:"));
}
