use meibo_core::Engine;
use serde_json::{Map, Value, json};

use super::tools;

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const CLIENT_KEY: &str = "io.modelcontextprotocol/clientInfo";

#[derive(Clone, Copy, PartialEq, Eq)]
enum LegacyState {
    New,
    Initialized,
    Ready,
}

pub(super) struct Protocol {
    allow_write: bool,
    legacy: LegacyState,
}

impl Protocol {
    pub(super) const fn new(allow_write: bool) -> Self {
        Self {
            allow_write,
            legacy: LegacyState::New,
        }
    }

    pub(super) fn handle(&mut self, engine: &mut Engine, message: &Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error(Value::Null, -32600, "Invalid request.", None));
        };
        let id = object.get("id");
        let valid_id = id.filter(|id| id.is_string() || id.is_i64() || id.is_u64());
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || !object.get("method").is_some_and(Value::is_string)
            || (id.is_some() && valid_id.is_none())
        {
            return Some(error(
                valid_id.cloned().unwrap_or(Value::Null),
                -32600,
                "Invalid request.",
                None,
            ));
        }
        let method = object["method"].as_str().expect("validated method");
        let Some(id) = valid_id.cloned() else {
            // Notifications cannot invoke business operations or produce responses.
            if method == "notifications/initialized"
                && self.legacy == LegacyState::Initialized
                && object.get("params").is_none_or(Value::is_object)
            {
                self.legacy = LegacyState::Ready;
            }
            return None;
        };
        let empty = Map::new();
        let params = match object.get("params") {
            None => &empty,
            Some(Value::Object(params)) => params,
            Some(_) => return Some(error(id, -32602, "Params must be an object.", None)),
        };
        if method == "initialize" {
            return Some(self.initialize(id, params));
        }
        if method == "ping" && params.is_empty() && self.legacy != LegacyState::New {
            return Some(json!({"jsonrpc":"2.0","id":id,"result":{}}));
        }
        let has_modern_version = params
            .get("_meta")
            .is_some_and(|meta| meta.get(VERSION_KEY).is_some());
        let modern = if has_modern_version || method == "server/discover" {
            let Some(meta) = params.get("_meta") else {
                return Some(error(id, -32602, "Per-request metadata is required.", None));
            };
            match validate_metadata(meta) {
                Ok(()) => true,
                Err((code, message, data)) => return Some(error(id, code, message, data)),
            }
        } else if self.legacy == LegacyState::Ready {
            if params.get("_meta").is_some_and(|meta| !meta.is_object()) {
                return Some(error(
                    id,
                    -32602,
                    "Request metadata must be an object.",
                    None,
                ));
            }
            false
        } else {
            return Some(error(
                id,
                -32602,
                "Include per-request metadata or initialize a legacy session.",
                None,
            ));
        };
        let result = self.operation(engine, method, params, modern);
        Some(match result {
            Ok(mut result) => {
                if modern {
                    result["resultType"] = json!("complete");
                    result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":server_info()});
                }
                json!({"jsonrpc":"2.0","id":id,"result":result})
            }
            Err((code, message)) => error(id, code, message, None),
        })
    }

    fn initialize(&mut self, id: Value, params: &Map<String, Value>) -> Value {
        if self.legacy != LegacyState::New
            || !params.get("protocolVersion").is_some_and(Value::is_string)
            || !params.get("capabilities").is_some_and(Value::is_object)
            || !params.get("clientInfo").is_some_and(valid_implementation)
        {
            return error(
                id,
                -32602,
                "Invalid initialization parameters or state.",
                None,
            );
        }
        self.legacy = LegacyState::Initialized;
        json!({"jsonrpc":"2.0","id":id,"result":{
            "protocolVersion":LEGACY,"capabilities":{"tools":{}},"serverInfo":server_info(),
            "instructions":"Read CRM data. Activity recording is available only with --allow-write. Do not automatically retry activity writes."
        }})
    }

    fn operation(
        &self,
        engine: &mut Engine,
        method: &str,
        params: &Map<String, Value>,
        modern: bool,
    ) -> Result<Value, (i64, &'static str)> {
        match method {
            "server/discover" => {
                only(params, &["_meta"])?;
                Ok(
                    json!({"supportedVersions":[MODERN],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}),
                )
            }
            "ping" => {
                only(params, &["_meta"])?;
                Ok(json!({}))
            }
            "tools/list" => {
                only(params, &["_meta"])?;
                let mut result = json!({"tools":tools::list(self.allow_write)});
                if modern {
                    result["ttlMs"] = json!(0);
                    result["cacheScope"] = json!("private");
                }
                Ok(result)
            }
            "tools/call" => {
                only(params, &["_meta", "name", "arguments"])?;
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or((-32602, "A tool name is required."))?;
                let empty = json!({});
                let arguments = params.get("arguments").unwrap_or(&empty);
                if !arguments.is_object() {
                    return Err((-32602, "Tool arguments must be an object."));
                }
                let (failed, value) =
                    crate::server::public_api::call_tool(engine, name, arguments, self.allow_write)
                        .map_err(|message| (-32602, message))?;
                Ok(
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":failed}),
                )
            }
            _ => Err((-32601, "Method not found.")),
        }
    }
}

type MetadataError = (i64, &'static str, Option<Value>);

fn validate_metadata(meta: &Value) -> Result<(), MetadataError> {
    let meta = meta
        .as_object()
        .ok_or((-32602, "Request metadata must be an object.", None))?;
    let version = meta
        .get(VERSION_KEY)
        .and_then(Value::as_str)
        .filter(|version| version.len() <= 64)
        .ok_or((-32602, "A protocol version is required.", None))?;
    if !meta.get(CAPABILITIES_KEY).is_some_and(Value::is_object)
        || meta
            .get(CLIENT_KEY)
            .is_some_and(|value| !valid_implementation(value))
    {
        return Err((
            -32602,
            "Invalid request capabilities or client identity.",
            None,
        ));
    }
    if version != MODERN {
        return Err((
            -32022,
            "Unsupported protocol version.",
            Some(json!({"supported":[MODERN],"requested":version})),
        ));
    }
    Ok(())
}

fn valid_implementation(value: &Value) -> bool {
    ["name", "version"].iter().all(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| {
                !value.trim().is_empty() && value.len() <= 200 && !value.contains('\0')
            })
    })
}

fn only(params: &Map<String, Value>, allowed: &[&str]) -> Result<(), (i64, &'static str)> {
    if params.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err((-32602, "Unsupported request parameters."))
    } else {
        Ok(())
    }
}

fn server_info() -> Value {
    json!({"name":"meibo","version":env!("CARGO_PKG_VERSION")})
}

pub(super) fn error(id: Value, code: i64, message: &'static str, data: Option<Value>) -> Value {
    let mut error = json!({"code":code,"message":message});
    if let Some(data) = data {
        error["data"] = data;
    }
    let mut response = Map::new();
    response.insert("jsonrpc".into(), json!("2.0"));
    response.insert("id".into(), id);
    response.insert("error".into(), error);
    Value::Object(response)
}
