//! Loopback HTTP flows over isolated synthetic databases.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use meibo_core::{
    Engine, OpenOptions,
    synth::{Profile, SynthConfig},
    types::DataOrigin,
};
use serde_json::Value;

struct Running {
    child: Child,
    address: String,
    _directory: tempfile::TempDir,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Running {
    fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("synthetic.db");
        {
            let mut engine =
                Engine::create(&database, DataOrigin::Synthetic, &OpenOptions::default()).unwrap();
            engine
                .load_synthetic(SynthConfig {
                    rows: 12,
                    seed: 42,
                    profile: Profile::Dense,
                })
                .unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_meibo"))
            .arg("serve")
            .arg(&database)
            .args(["--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
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
            _directory: directory,
        };
        let line = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("server startup timed out");
        running.address = line
            .trim()
            .strip_prefix("http://")
            .expect("server URL")
            .trim_end_matches('/')
            .into();
        running
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        host: &str,
        headers: &str,
        body: &str,
    ) -> (u16, String, String) {
        let mut connection = TcpStream::connect(&self.address).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(connection, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
        connection.flush().unwrap();
        let mut response = Vec::new();
        connection.read_to_end(&mut response).unwrap();
        let boundary = response
            .windows(4)
            .position(|v| v == b"\r\n\r\n")
            .expect("HTTP response");
        let head = String::from_utf8(response[..boundary].to_vec()).unwrap();
        let raw = &response[boundary + 4..];
        let body = if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            decode_chunks(raw)
        } else {
            raw.to_vec()
        };
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, head, String::from_utf8(body).unwrap())
    }

    fn get(&self, path: &str) -> (u16, String, String) {
        self.request("GET", path, &self.address, "", "")
    }

    fn json(&self, path: &str) -> Value {
        let (status, _, body) = self.get(path);
        assert_eq!(status, 200, "{body}");
        serde_json::from_str(&body).unwrap()
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
            return body;
        }
        raw = &raw[boundary + 2..];
        body.extend_from_slice(&raw[..size]);
        assert_eq!(&raw[size..size + 2], b"\r\n");
        raw = &raw[size + 2..];
    }
}

// REQ 15.1, REQ 16.1, REQ 16.2, REQ 16.4
#[test]
fn browser_assets_lists_cursors_details_and_csv_are_served() {
    let running = Running::start();
    assert_eq!(running.json("/health")["ok"], true);
    let (status, headers, html) = running.get("/");
    assert_eq!(status, 200);
    assert!(html.contains("meibo"));
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-security-policy:")
    );
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("x-content-type-options: nosniff")
    );
    for path in ["/style.css", "/app.js"] {
        assert_eq!(running.get(path).0, 200);
    }
    let summary = running.json("/api/summary");
    assert_eq!(summary["origin"], "synthetic");
    assert_eq!(summary["total"], 12);
    let first = running.json("/api/companies?limit=1");
    assert_eq!(first["rows"].as_array().unwrap().len(), 1);
    let second = running.json(&format!(
        "/api/companies?limit=1&after={}",
        first["next"].as_str().unwrap()
    ));
    assert_ne!(first["rows"][0]["id"], second["rows"][0]["id"]);
    let id = first["rows"][0]["id"].as_i64().unwrap();
    assert_eq!(
        running.json(&format!("/api/companies/{id}"))["company"]["id"],
        id
    );
    assert_eq!(
        running.json("/api/companies?q=nonexistent-synthetic-name")["rows"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let (status, headers, csv) = running.get("/api/export");
    assert_eq!(status, 200);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-disposition:")
    );
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("corporate_number"));
    assert_eq!(running.get("/not-found").0, 404);
}

// REQ 15.2, REQ 16.3
#[test]
fn only_same_origin_json_can_save_an_activity() {
    let running = Running::start();
    let id = running.json("/api/companies?limit=1")["rows"][0]["id"]
        .as_i64()
        .unwrap();
    let path = format!("/api/companies/{id}/activities");
    let body = r#"{"note":"合成ローカル起動テスト","status":2,"next_action_at":2000000000}"#;
    let headers = format!(
        "Origin: http://{}\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\n",
        running.address
    );
    let (status, _, _) = running.request("POST", &path, &running.address, &headers, body);
    assert_eq!(status, 200);
    let detail = running.json(&format!("/api/companies/{id}"));
    assert_eq!(detail["company"]["status"], 2);
    assert_eq!(detail["company"]["next_action_at"], 2_000_000_000);
    assert!(
        detail["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["note"] == "合成ローカル起動テスト")
    );
    let before = detail["activities"].as_array().unwrap().len();
    for (host, bad_headers, bad_body) in [
        ("attacker.invalid", headers.as_str(), body),
        (
            running.address.as_str(),
            "Origin: https://attacker.invalid\r\nContent-Type: application/json\r\nX-Meibo-Local: 1\r\n",
            body,
        ),
        (
            running.address.as_str(),
            "Content-Type: application/json\r\n",
            body,
        ),
        (
            running.address.as_str(),
            headers.as_str(),
            r#"{"status":6}"#,
        ),
        (
            running.address.as_str(),
            headers.as_str(),
            r#"{"next_action_at":-1}"#,
        ),
    ] {
        assert!(
            running
                .request("POST", &path, host, bad_headers, bad_body)
                .0
                >= 400
        );
    }
    let detail = running.json(&format!("/api/companies/{id}"));
    assert_eq!(detail["activities"].as_array().unwrap().len(), before);
    assert!(
        running
            .request("GET", "/api/summary", "attacker.invalid", "", "")
            .0
            >= 400
    );
}

// REQ 18.1, REQ 18.2, REQ 18.3: server-side views retain their scope across pages.
#[test]
fn workspace_views_summary_and_search_scope_are_consistent() {
    let running = Running::start();
    let summary = running.json("/api/summary");
    let statuses = summary["status_counts"].as_array().unwrap();
    assert_eq!(statuses.len(), 6);
    assert_eq!(
        statuses.iter().map(|n| n.as_u64().unwrap()).sum::<u64>(),
        12
    );
    let untouched = running.json("/api/companies?view=untouched&limit=1");
    assert_eq!(untouched["rows"][0]["status"], 0);
    if let Some(next) = untouched["next"].as_str() {
        let next = running.json(&format!(
            "/api/companies?view=untouched&limit=1&after={next}"
        ));
        assert_eq!(next["rows"][0]["status"], 0);
    }
    let scheduled = running.json("/api/companies?view=scheduled");
    let dates = scheduled["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["next_action_at"].as_i64().expect("scheduled date"))
        .collect::<Vec<_>>();
    assert!(dates.windows(2).all(|pair| pair[0] <= pair[1]));
    for path in [
        "/api/companies?view=unknown",
        "/api/companies?status=8",
        "/api/companies?prefecture=48",
        "/api/companies?has_phone=maybe",
        "/api/companies?q=synthetic&view=untouched",
        "/api/companies?q=synthetic&status=0",
    ] {
        assert_eq!(running.get(path).0, 400, "{path}");
    }
}

// REQ 18.6: a new synthetic account persists, duplicates and cross-origin writes fail.
#[test]
fn account_creation_returns_an_id_and_keeps_validation_and_http_boundaries() {
    let running = Running::start();
    let headers = format!(
        "Origin: http://{}\r\nX-Meibo-Local: 1\r\nContent-Type: application/json\r\n",
        running.address
    );
    let duplicate = r#"{"name":"合成重複見本","prefecture":13,"corporate_number":"6600000000000"}"#;
    assert_eq!(
        running
            .request(
                "POST",
                "/api/companies",
                &running.address,
                &headers,
                duplicate
            )
            .0,
        409
    );
    let body = r#"{"name":"合成ワークスペース見本","prefecture":13,"phone":"0300000000","website":"https://example.invalid"}"#;
    let (status, _, response) =
        running.request("POST", "/api/companies", &running.address, &headers, body);
    assert_eq!(status, 201, "{response}");
    let id = serde_json::from_str::<Value>(&response).unwrap()["id"]
        .as_i64()
        .unwrap();
    let record = running.json(&format!("/api/companies/{id}"));
    assert_eq!(record["company"]["name"], "合成ワークスペース見本");
    assert_eq!(record["company"]["status"], 0);
    assert_eq!(running.json("/api/summary")["total"], 13);
    for body in [
        r#"{"name":"","prefecture":13}"#,
        r#"{"name":"合成不正","prefecture":48}"#,
        r#"{"name":"合成不正","prefecture":13,"origin":"real"}"#,
    ] {
        assert_eq!(
            running
                .request("POST", "/api/companies", &running.address, &headers, body)
                .0,
            400
        );
    }
    assert!(
        running
            .request(
                "POST",
                "/api/companies",
                &running.address,
                "Content-Type: application/json\r\n",
                body
            )
            .0
            >= 400
    );
    assert_eq!(running.json("/api/summary")["total"], 13);
}
