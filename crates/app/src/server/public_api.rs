//! Versioned HTTP operations shared by local integrations and MCP.
use meibo_core::Engine;
use serde_json::{Map, Value, json};

use super::{Input, MAX_BODY, Reply, api, company_id};

pub(crate) fn dispatch(
    engine: &mut Engine,
    input: &Input,
    path: &str,
    query: &str,
) -> Option<Reply> {
    if path != "/api/v1" && !path.starts_with("/api/v1/") {
        return None;
    }
    let expected = if path == "/api/v1/companies" {
        "GET"
    } else if let Some(rest) = path.strip_prefix("/api/v1/companies/") {
        let (raw_id, action) = rest.split_once('/').unwrap_or((rest, ""));
        let Some(id) = company_id(raw_id) else {
            return Some(Reply::error(404, "対象が見つかりません。"));
        };
        return Some(match action {
            "" if input.method == "GET" => api::detail(engine, id, query),
            "activities" if input.method == "POST" && query.is_empty() => {
                api::activity(engine, id, &input.body)
            }
            "activities" if input.method == "POST" => {
                Reply::error(400, "活動記録には検索条件を指定できません。")
            }
            "" | "activities" => Reply::error(405, "この操作には対応していません。"),
            _ => Reply::error(404, "対象が見つかりません。"),
        });
    } else {
        return Some(Reply::error(404, "対象が見つかりません。"));
    };
    Some(if input.method == expected {
        api::companies(engine, query)
    } else {
        Reply::error(405, "この操作には対応していません。")
    })
}

/// Maps fixed tool names into the same versioned operations as HTTP.
/// `Err` indicates an unknown/disabled tool; operation validation is a tool error.
pub(crate) fn call_tool(
    engine: &mut Engine,
    name: &str,
    arguments: &Value,
    allow_write: bool,
) -> Result<(bool, Value), &'static str> {
    if !(matches!(name, "list_companies" | "search_companies" | "get_company")
        || name == "record_activity" && allow_write)
    {
        return Err("Unknown or disabled tool.");
    }
    let invalid = || (true, json!({"error":"引数の項目と型を確認してください。"}));
    let Some(arguments) = arguments.as_object() else {
        return Ok(invalid());
    };
    let Some((method, target, body)) = tool_request(name, arguments) else {
        return Ok(invalid());
    };
    if body.len() > MAX_BODY {
        return Ok((true, json!({"error":"送信内容が上限を超えています。"})));
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let input = Input {
        method: method.into(),
        target: target.clone(),
        headers: vec![],
        body,
    };
    let reply = dispatch(engine, &input, path, query).expect("fixed v1 tool route");
    let value = serde_json::from_slice(&reply.body)
        .unwrap_or_else(|_| json!({"error":"処理結果を読み取れませんでした。"}));
    Ok((reply.status >= 400, value))
}

fn tool_request(
    name: &str,
    arguments: &Map<String, Value>,
) -> Option<(&'static str, String, Vec<u8>)> {
    match name {
        "list_companies" | "search_companies" => {
            let mut pairs = Vec::new();
            let searching = name == "search_companies";
            for (key, value) in arguments {
                let text = match key.as_str() {
                    "query" if searching => {
                        let value = value.as_str()?.trim();
                        if value.is_empty() || value.len() > 4096 || value.contains('\0') {
                            return None;
                        }
                        value.to_owned()
                    }
                    "limit" => bounded_integer(value, 1, 200)?.to_string(),
                    "after" => {
                        let value = value.as_str()?;
                        if value.len() > 4096 || value.contains('\0') {
                            return None;
                        }
                        value.to_owned()
                    }
                    "status" if !searching => bounded_integer(value, 0, 4)?.to_string(),
                    "prefecture" if !searching => bounded_integer(value, 1, 47)?.to_string(),
                    "has_phone" if !searching => value.as_bool()?.to_string(),
                    "view" | "sort" if !searching => value.as_str()?.to_owned(),
                    _ => return None,
                };
                pairs.push(format!(
                    "{}={}",
                    if key == "query" { "q" } else { key },
                    encode(&text)
                ));
            }
            if searching && !arguments.contains_key("query") {
                return None;
            }
            Some((
                "GET",
                format!("/api/v1/companies?{}", pairs.join("&")),
                vec![],
            ))
        }
        "get_company" => {
            let id = arguments.get("id")?.as_i64().filter(|id| *id > 0)?;
            let mut pairs = Vec::new();
            for (key, value) in arguments {
                match key.as_str() {
                    "id" => {}
                    "include_contacts" | "include_private" => {
                        if value.as_bool()? {
                            pairs.push(format!("{key}=1"));
                        }
                    }
                    _ => return None,
                }
            }
            Some((
                "GET",
                format!("/api/v1/companies/{id}?{}", pairs.join("&")),
                vec![],
            ))
        }
        "record_activity" => {
            let id = arguments.get("company_id")?.as_i64().filter(|id| *id > 0)?;
            let mut body = arguments.clone();
            body.remove("company_id");
            Some((
                "POST",
                format!("/api/v1/companies/{id}/activities"),
                Value::Object(body).to_string().into_bytes(),
            ))
        }
        _ => None,
    }
}

fn bounded_integer(value: &Value, minimum: u64, maximum: u64) -> Option<u64> {
    value
        .as_u64()
        .filter(|value| (minimum..=maximum).contains(value))
}

fn encode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests;
