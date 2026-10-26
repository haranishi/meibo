use super::*;
use meibo_core::{
    OpenOptions,
    types::{CompanyInput, DataOrigin, Status},
};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, Engine, i64) {
    let directory = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &directory.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let id = engine
        .register_company(&CompanyInput {
            origin: DataOrigin::Synthetic,
            corporate_number: None,
            name: "合成MCP企業".into(),
            prefecture: 13,
            municipality: None,
            industry: None,
            phone: None,
            website: None,
            owner: None,
            status: Status::Untouched,
            is_sole_proprietor: false,
            source_id: Some("synthetic-source".into()),
            source_as_of: Some("2026-10-26".into()),
        })
        .unwrap();
    (directory, engine, id.0)
}

fn modern(method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{},
        "io.modelcontextprotocol/clientInfo":{"name":"synthetic-test","version":"1"}
    });
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}

#[test]
fn modern_discover_and_calls_work_without_a_handshake() {
    let (_directory, mut engine, _) = fixture();
    let mut protocol = protocol::Protocol::new(false);
    let response = protocol
        .handle(&mut engine, &modern("server/discover", json!({})))
        .unwrap();
    assert_eq!(response["result"]["resultType"], "complete");
    assert_eq!(
        response["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    assert_eq!(
        response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "meibo"
    );
    assert_eq!(response["result"]["ttlMs"], 0);
    assert_eq!(response["result"]["cacheScope"], "private");
    let tools = protocol
        .handle(&mut engine, &modern("tools/list", json!({})))
        .unwrap();
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(tools["result"]["cacheScope"], "private");
    let call = protocol
        .handle(
            &mut engine,
            &modern(
                "tools/call",
                json!({"name":"list_companies","arguments":{}}),
            ),
        )
        .unwrap();
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(
        call["result"]["structuredContent"]["rows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn modern_metadata_and_versions_are_validated_on_every_request() {
    let (_directory, mut engine, _) = fixture();
    let mut protocol = protocol::Protocol::new(false);
    let mut unsupported = modern("tools/list", json!({}));
    unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("1900-01-01");
    let response = protocol.handle(&mut engine, &unsupported).unwrap();
    assert_eq!(response["error"]["code"], -32022);
    assert_eq!(
        response["error"]["data"]["supported"],
        json!(["2026-07-28"])
    );
    let mut missing = modern("tools/list", json!({}));
    missing["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    assert_eq!(
        protocol.handle(&mut engine, &missing).unwrap()["error"]["code"],
        -32602
    );
    let missing_all = json!({"jsonrpc":"2.0","id":3,"method":"server/discover","params":{}});
    assert_eq!(
        protocol.handle(&mut engine, &missing_all).unwrap()["error"]["code"],
        -32602
    );
    assert_eq!(
        protocol
            .handle(
                &mut engine,
                &json!({"jsonrpc":"2.0","id":4,"method":"ping"})
            )
            .unwrap()["error"]["code"],
        -32602
    );
}

#[test]
fn legacy_initialize_requires_ready_notification_and_offers_legacy_version() {
    let (_directory, mut engine, _) = fixture();
    let mut protocol = protocol::Protocol::new(false);
    let listing = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
    assert!(
        protocol
            .handle(&mut engine, &listing)
            .unwrap()
            .get("error")
            .is_some()
    );
    let initialized = protocol.handle(&mut engine, &json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"legacy-test","version":"1"}}
    })).unwrap();
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert!(
        protocol
            .handle(&mut engine, &listing)
            .unwrap()
            .get("error")
            .is_some()
    );
    assert!(
        protocol
            .handle(
                &mut engine,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .is_none()
    );
    let result = protocol.handle(&mut engine, &listing).unwrap();
    assert_eq!(result["result"]["tools"].as_array().unwrap().len(), 3);
    assert!(result["result"].get("resultType").is_none());
    let metadata = json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"progressToken":"synthetic-token"}}});
    assert_eq!(
        protocol.handle(&mut engine, &metadata).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn read_only_tools_cannot_write_even_when_called_directly() {
    let (_directory, mut engine, id) = fixture();
    let mut protocol = protocol::Protocol::new(false);
    let request = modern(
        "tools/call",
        json!({"name":"record_activity","arguments":{"company_id":id,"note":"must not save"}}),
    );
    assert!(
        protocol
            .handle(&mut engine, &request)
            .unwrap()
            .get("error")
            .is_some()
    );
    assert_eq!(
        engine
            .company_detail(meibo_core::types::CompanyId(id), &vec![])
            .unwrap()
            .activities,
        []
    );
    let mut writable = protocol::Protocol::new(true);
    let listing = writable
        .handle(&mut engine, &modern("tools/list", json!({})))
        .unwrap();
    let record = listing["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "record_activity")
        .unwrap();
    assert_eq!(record["annotations"]["readOnlyHint"], false);
    assert_eq!(record["annotations"]["idempotentHint"], false);
    assert_eq!(record["annotations"]["openWorldHint"], false);
    let result = writable.handle(&mut engine, &request).unwrap();
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(
        engine
            .company_detail(meibo_core::types::CompanyId(id), &vec![])
            .unwrap()
            .activities
            .len(),
        1
    );
}

#[test]
fn tool_failures_are_results_and_protocol_failures_are_errors() {
    let (_directory, mut engine, _) = fixture();
    let mut protocol = protocol::Protocol::new(false);
    let invalid = modern(
        "tools/call",
        json!({"name":"list_companies","arguments":{"limit":"not an integer"}}),
    );
    assert_eq!(
        protocol.handle(&mut engine, &invalid).unwrap()["result"]["isError"],
        true
    );
    assert_eq!(
        protocol
            .handle(&mut engine, &modern("unknown/method", json!({})))
            .unwrap()["error"]["code"],
        -32601
    );
    assert_eq!(
        protocol
            .handle(
                &mut engine,
                &modern("tools/call", json!({"name":"unknown_tool","arguments":{}}))
            )
            .unwrap()["error"]["code"],
        -32602
    );
    assert_eq!(
        protocol
            .handle(
                &mut engine,
                &modern("tools/list", json!({"cursor":"unknown"}))
            )
            .unwrap()["error"]["code"],
        -32602
    );
    assert_eq!(
        protocol.handle(&mut engine, &json!([])).unwrap()["error"]["code"],
        -32600
    );
    assert_eq!(
        protocol
            .handle(
                &mut engine,
                &json!({"jsonrpc":"1.0","id":1,"method":"ping"})
            )
            .unwrap()["error"]["code"],
        -32600
    );
}

#[test]
fn notifications_never_reply_or_execute_business_writes() {
    let (_directory, mut engine, id) = fixture();
    let mut protocol = protocol::Protocol::new(true);
    let mut notification = modern(
        "tools/call",
        json!({"name":"record_activity","arguments":{"company_id":id,"note":"must not save"}}),
    );
    notification.as_object_mut().unwrap().remove("id");
    assert!(protocol.handle(&mut engine, &notification).is_none());
    assert_eq!(
        engine
            .company_detail(meibo_core::types::CompanyId(id), &vec![])
            .unwrap()
            .activities,
        []
    );
}

#[test]
fn stdio_recovers_from_parse_errors_and_oversized_lines_then_exits_at_eof() {
    let (_directory, mut engine, _) = fixture();
    let mut input = b"invalid-json\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', MAX_MESSAGE + 1));
    input.push(b'\n');
    input.extend(serde_json::to_vec(&modern("ping", json!({}))).unwrap());
    let mut output = Vec::new();
    run_io(&mut engine, std::io::Cursor::new(input), &mut output, false).unwrap();
    let lines = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["error"]["code"], -32600);
    assert_eq!(lines[2]["result"]["resultType"], "complete");
    assert!(output.len() < 2048);
}
