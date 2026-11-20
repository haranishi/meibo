//! 公開の問い合わせ受付と、組織内の引き継ぎ操作。
use meibo_core::{
    Engine,
    leads::{Conversion, Inquiry, valid_key},
    types::EngineError,
};
use serde_json::{Value, json};
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Input, Reply, api};

pub(crate) const FORM_LIMIT: usize = 8192;

pub(crate) fn form_path(path: &str) -> Option<(&str, &str)> {
    let (tenant, key) = path.strip_prefix("/forms/")?.split_once('/')?;
    if !(1..=48).contains(&tenant.len())
        || tenant.starts_with('-')
        || tenant.ends_with('-')
        || !tenant
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || !valid_key(key)
    {
        return None;
    }
    Some((tenant, key))
}

pub(crate) fn public_asset(path: &str) -> Option<Reply> {
    match path {
        "/forms/intake.js" => Some(Reply::text(
            "text/javascript; charset=utf-8",
            include_str!("../../ui/lead-form.js"),
        )),
        "/forms/intake.css" => Some(Reply::text(
            "text/css; charset=utf-8",
            include_str!("../../ui/lead-form.css"),
        )),
        _ => None,
    }
}

fn missing() -> Reply {
    Reply::error(404, "この受付フォームは利用できません。")
}
fn bad() -> Reply {
    Reply::error(400, "入力内容と同意の選択を確認してください。")
}
fn local_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

pub(crate) fn public_form(engine: &mut Engine, key: &str, input: &Input, at: i64) -> Reply {
    match engine.lead_form() {
        Ok(Some(active)) if active == key => {}
        Ok(_) => return missing(),
        Err(e) => return api::engine_error(&e),
    }
    match input.method.as_str() {
        "GET" => {
            let html = include_str!("../../ui/lead-form.html");
            let mut page = Reply::text("text/html; charset=utf-8", html);
            if engine.origin() == meibo_core::types::DataOrigin::Synthetic {
                page.body = html
                    .replace(
                        "<!-- origin-notice -->",
                        "<p>デモ用フォームです。架空の名前とメールアドレスを入力してください。</p>",
                    )
                    .into_bytes();
            }
            page
        }
        "POST" => {
            if input.body.len() > FORM_LIMIT {
                return Reply::error(413, "送信内容が上限を超えています。");
            }
            let Ok(inquiry) = serde_json::from_slice::<Inquiry>(&input.body) else {
                return bad();
            };
            match engine.receive_lead(key, &inquiry, at) {
                Ok(()) => Reply::json(202, &json!({"accepted":true})),
                Err(EngineError::RevisionConflict) => {
                    Reply::error(409, "先に送信した内容を受け付けています。")
                }
                Err(EngineError::NotFound) => missing(),
                Err(EngineError::InvalidInput { field, .. }) if field == "inbox_capacity" => {
                    Reply::error(
                        429,
                        "現在は受け付けられません。時間を置いてお試しください。",
                    )
                }
                Err(EngineError::InvalidInput { .. }) => bad(),
                Err(e) => api::engine_error(&e),
            }
        }
        _ => Reply::error(405, "この操作には対応していません。"),
    }
}

fn change_form(engine: &mut Engine, input: &Input) -> Result<Value, Reply> {
    let body: Value = serde_json::from_slice(&input.body).map_err(|_| bad())?;
    let enabled = body
        .get("enabled")
        .and_then(Value::as_bool)
        .filter(|_| body.as_object().is_some_and(|obj| obj.len() == 1))
        .ok_or_else(bad)?;
    let key = if enabled {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|_| Reply::error(503, "受付URLを発行できませんでした。"))?;
        let mut key = String::with_capacity(32);
        for b in bytes {
            write!(key, "{b:02x}").expect("writing a String succeeds");
        }
        Some(key)
    } else {
        None
    };
    engine
        .set_lead_form(key.as_deref())
        .map(|public_key| json!({"public_key":public_key}))
        .map_err(|e| api::engine_error(&e))
}

fn list(engine: &Engine, query: &str) -> Result<Value, Reply> {
    let mut state = None;
    let mut after = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        match pair.split_once('=') {
            Some(("state", v)) if state.is_none() => {
                state = Some(match v {
                    "open" => 0,
                    "converted" => 1,
                    "discarded" => 2,
                    _ => return Err(bad()),
                });
            }
            Some(("after", v)) if after.is_none() => {
                after = Some(super::company_id(v).ok_or_else(bad)?.0);
            }
            _ => return Err(bad()),
        }
    }
    let mut items = engine
        .leads(state.unwrap_or(0), after)
        .map_err(|e| api::engine_error(&e))?;
    let more = items.len() > 50;
    items.truncate(50);
    Ok(json!({"next":if more {items.last().map(|i|i.id)} else{None}, "items":items}))
}

fn lead_error(error: &EngineError) -> Reply {
    match error {
        EngineError::RevisionConflict => Reply::error(
            409,
            "受付内容が更新されています。一覧を読み直してください。",
        ),
        EngineError::InvalidInput { field, .. } if field == "existing_company" => Reply::error(
            409,
            "同じ都道府県に同名の取引先があります。「既存の取引先」から選んでください。",
        ),
        _ => api::engine_error(error),
    }
}

fn write_lead(engine: &mut Engine, id: i64, action: &str, input: &Input) -> Result<Value, Reply> {
    let body: Value = serde_json::from_slice(&input.body).map_err(|_| bad())?;
    let obj = body.as_object().ok_or_else(bad)?;
    let revision = obj
        .get("revision")
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .ok_or_else(bad)?;
    let outcome = match action {
        "assign" if obj.len() == 2 => {
            let owner = match obj.get("owner_id") {
                Some(Value::Null) => None,
                Some(n) => Some(n.as_i64().filter(|v| *v > 0).ok_or_else(bad)?),
                None => return Err(bad()),
            };
            engine.assign_lead(id, revision, owner)
        }
        "discard" if obj.len() == 1 => engine.discard_lead(id, revision),
        "convert" => {
            let conversion: Conversion = serde_json::from_value(body).map_err(|_| bad())?;
            engine.convert_lead(id, &conversion)
        }
        _ => return Err(bad()),
    };
    outcome.map(|item| json!(item)).map_err(|e| lead_error(&e))
}

pub(super) fn dispatch(
    engine: &mut Engine,
    input: &Input,
    path: &str,
    query: &str,
) -> Option<Reply> {
    if path.starts_with("/forms/") {
        if input.target != path {
            return Some(missing());
        }
        if input.method == "GET"
            && let Some(asset) = public_asset(path)
        {
            return Some(asset);
        }
        return Some(match form_path(path) {
            Some(("local", key)) => public_form(engine, key, input, local_now()),
            _ => missing(),
        });
    }
    let result = match (input.method.as_str(), path) {
        ("GET", "/api/lead-form") if query.is_empty() => engine
            .lead_form()
            .map(|public_key| json!({"public_key":public_key}))
            .map_err(|e| api::engine_error(&e)),
        ("POST", "/api/lead-form") if query.is_empty() => change_form(engine, input),
        ("GET", "/api/leads") => list(engine, query),
        _ => {
            let rest = path.strip_prefix("/api/leads/")?;
            let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
            let id = super::company_id(id)?.0;
            match (input.method.as_str(), action) {
                ("GET", "") if matches!(query, "" | "reveal=1") => engine
                    .lead(id, query == "reveal=1")
                    .map(|lead| json!(lead))
                    .map_err(|e| api::engine_error(&e)),
                ("POST", "assign" | "discard" | "convert") if query.is_empty() => {
                    write_lead(engine, id, action, input)
                }
                _ => return Some(bad()),
            }
        }
    };
    Some(result.map_or_else(|reply| reply, |v| Reply::json(200, &v)))
}
