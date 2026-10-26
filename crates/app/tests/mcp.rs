//! MCP process framing and persistent operations over synthetic databases.
use meibo_core::{
    Engine, OpenOptions,
    types::{CompanyInput, DataOrigin, Status},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, i64) {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("synthetic.db");
    let mut engine = Engine::create(&db, DataOrigin::Synthetic, &OpenOptions::default()).unwrap();
    let id = engine
        .register_company(&CompanyInput {
            origin: DataOrigin::Synthetic,
            corporate_number: None,
            name: "合成MCP試験企業".into(),
            prefecture: 13,
            municipality: None,
            industry: None,
            phone: None,
            website: None,
            owner: None,
            status: Status::Untouched,
            is_sole_proprietor: false,
            source_id: Some("nta".into()),
            source_as_of: Some("2026-10-26".into()),
        })
        .unwrap();
    (directory, db, id.0)
}

fn request(id: i64, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{}
    });
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn run(db: &std::path::Path, writable: bool, messages: &[Value]) -> Vec<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_meibo"));
    command.arg("mcp").arg(db);
    if writable {
        command.arg("--allow-write");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for message in messages {
            serde_json::to_writer(&mut stdin, message).unwrap();
            stdin.write_all(b"\n").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, [] as [u8; 0]);
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("stdout contains only JSON-RPC"))
        .collect()
}

#[test]
fn modern_process_exits_at_eof_and_keeps_stdout_protocol_only() {
    let (_directory, db, id) = fixture();
    let output = run(
        &db,
        false,
        &[
            request(1, "server/discover", json!({})),
            request(
                2,
                "tools/call",
                json!({"name":"get_company","arguments":{"id":id}}),
            ),
        ],
    );
    assert_eq!(output.len(), 2);
    assert_eq!(
        output[0]["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    assert_eq!(
        output[1]["result"]["structuredContent"]["company"]["id"],
        id
    );
    assert_eq!(output[1]["result"]["isError"], false);
    let company = &output[1]["result"]["structuredContent"]["company"];
    assert_eq!(company["source_id"], "nta");
    assert_eq!(company["source_as_of"], "2026-10-26");
    assert_ne!(company["attribution"].as_str().unwrap(), "");
    assert_ne!(company["modification_notice"].as_str().unwrap(), "");
}

#[test]
fn legacy_process_notifies_without_an_extra_reply_and_stays_read_only() {
    let (_directory, db, id) = fixture();
    let output = run(
        &db,
        false,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic-test","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"record_activity","arguments":{"company_id":id,"note":"must not save"}}}),
        ],
    );
    assert_eq!(output.len(), 3);
    assert_eq!(output[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(output[1]["result"]["tools"].as_array().unwrap().len(), 3);
    assert!(output[2].get("error").is_some());
    let engine = Engine::open(&db, &OpenOptions::default()).unwrap();
    assert_eq!(
        engine
            .company_detail(meibo_core::types::CompanyId(id), &vec![])
            .unwrap()
            .activities,
        []
    );
}

#[test]
fn explicitly_enabled_activity_write_survives_process_restart() {
    let (_directory, db, id) = fixture();
    let output = run(
        &db,
        true,
        &[request(
            1,
            "tools/call",
            json!({"name":"record_activity","arguments":{"company_id":id,"note":"合成の架電メモ","kind":1,"next_action_at":1_900_000_000}}),
        )],
    );
    assert_eq!(output[0]["result"]["isError"], false);
    let engine = Engine::open(&db, &OpenOptions::default()).unwrap();
    let detail = engine
        .company_detail(meibo_core::types::CompanyId(id), &vec![])
        .unwrap();
    assert_eq!(detail.activities.len(), 1);
    assert_eq!(detail.company.next_action_at, Some(1_900_000_000));
}

#[test]
fn missing_database_fails_without_creating_a_file_or_printing_a_path() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("private-missing.db");
    let output = Command::new(env!("CARGO_BIN_EXE_meibo"))
        .arg("mcp")
        .arg(&db)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout, [] as [u8; 0]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-missing"));
    assert!(!db.exists());
}
