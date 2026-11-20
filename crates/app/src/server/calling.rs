//! 共有架電リストは、この要求で既に選ばれた組織のEngineだけを使う。
use meibo_core::{Engine, calling::CallingConditions, types::EngineError};
use serde_json::{Value, json};

use super::{Input, Reply, api};

fn positive(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
}

pub(super) fn dispatch(
    engine: &mut Engine,
    input: &Input,
    path: &str,
    query: &str,
) -> Option<Reply> {
    if input.method == "GET"
        && let Some(id) = path
            .strip_prefix("/api/call-candidates/")
            .and_then(super::company_id)
    {
        return Some(api::call_candidate(engine, id, query));
    }
    let suffix = path.strip_prefix("/api/calling-lists")?;
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return None;
    }
    let bad = || Reply::error(400, "架電リスト名と保存する条件を確認してください。");
    if !query.is_empty() {
        return Some(bad());
    }
    if input.method == "GET" && suffix.is_empty() {
        return Some(match engine.calling_lists() {
            Ok(lists) => Reply::json(200, &json!({"lists":lists})),
            Err(err) => api::engine_error(&err),
        });
    }
    if input.method != "POST" {
        return None;
    }
    let (id, delete) = match suffix.split('/').collect::<Vec<_>>().as_slice() {
        [""] => (None, false),
        ["", id] | ["", id, "delete"] => {
            let id = id.parse::<u32>().ok().filter(|id| *id > 0)?;
            (Some(id), suffix.ends_with("/delete"))
        }
        _ => return None,
    };
    let Ok(Value::Object(mut body)) = serde_json::from_slice(&input.body) else {
        return Some(bad());
    };
    let existing = if let Some(id) = id {
        let Some(expected) = body.remove("revision").as_ref().and_then(positive) else {
            return Some(bad());
        };
        Some((id, expected))
    } else {
        None
    };
    let result = if let Some((id, revision)) = existing.filter(|_| delete) {
        if !body.is_empty() {
            return Some(bad());
        }
        engine
            .delete_calling_list(id, revision)
            .map(|()| json!({"ok":true}))
    } else {
        let Some(Value::String(name)) = body.remove("name") else {
            return Some(bad());
        };
        let Some(conditions) = body
            .remove("conditions")
            .and_then(|v| serde_json::from_value::<CallingConditions>(v).ok())
        else {
            return Some(bad());
        };
        if !body.is_empty() {
            return Some(bad());
        }
        engine
            .save_calling_list(existing, &name, conditions)
            .map(|list| json!(list))
    };
    Some(match result {
        Ok(value) => Reply::json(if existing.is_some() { 200 } else { 201 }, &value),
        Err(EngineError::RevisionConflict) => Reply::error(
            409,
            "この架電リストは別の操作で更新または削除されました。読み直してから保存してください。",
        ),
        Err(EngineError::InvalidInput { field, .. }) if field == "lists" => Reply::error(
            400,
            "共有リストの保存上限に達しました。不要なリストを削除してください。",
        ),
        Err(EngineError::InvalidInput { .. } | EngineError::InvalidSearchTerm) => bad(),
        Err(err) => api::engine_error(&err),
    })
}
