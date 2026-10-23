//! ToDo・予定・商談のJSON経路。呼出元がHTTP境界と容量を検査する。

use std::collections::HashMap;

use meibo_core::{
    Engine,
    crm::{
        Opportunity, OpportunityInput, OpportunityQuery, WorkItem, WorkItemInput, WorkKind,
        WorkQuery,
    },
    types::{CompanyId, ContactId, EngineError},
};
use serde_json::{Map, Value, json};

use super::{Input, Reply};

pub(super) fn dispatch(
    engine: &mut Engine,
    input: &Input,
    path: &str,
    query: &str,
) -> Option<Reply> {
    match path {
        "/api/work-items" => Some(match input.method.as_str() {
            "GET" => work_list(engine, query),
            "POST" if query.is_empty() => work_save(engine, &input.body, None),
            _ => bad_input(),
        }),
        "/api/opportunities" => Some(match input.method.as_str() {
            "GET" => opportunity_list(engine, query),
            "POST" if query.is_empty() => opportunity_save(engine, &input.body, None),
            _ => bad_input(),
        }),
        _ => record_route(engine, input, path, query),
    }
}

fn record_route(engine: &mut Engine, input: &Input, path: &str, query: &str) -> Option<Reply> {
    if let Some(rest) = path.strip_prefix("/api/work-items/") {
        return Some(if input.method != "POST" || !query.is_empty() {
            bad_input()
        } else if let Some(id) = rest.strip_suffix("/completion").and_then(positive) {
            let result = object(&input.body, &["completed"])
                .and_then(|o| o.get("completed").and_then(Value::as_bool));
            match result {
                Some(done) => saved(engine.crm_set_completion(id, done).map(|()| id), false),
                None => bad_input(),
            }
        } else if let Some(id) = positive(rest) {
            work_save(engine, &input.body, Some(id))
        } else {
            bad_input()
        });
    }
    if let Some(rest) = path.strip_prefix("/api/opportunities/") {
        return Some(if input.method != "POST" || !query.is_empty() {
            bad_input()
        } else if let Some(id) = rest.strip_suffix("/stage").and_then(positive) {
            let stage = object(&input.body, &["stage"])
                .and_then(|o| o.get("stage").and_then(Value::as_u64))
                .and_then(|v| u8::try_from(v).ok())
                .filter(|v| *v <= 5);
            match stage {
                Some(stage) => saved(engine.crm_set_stage(id, stage).map(|()| id), false),
                None => bad_input(),
            }
        } else if let Some(id) = positive(rest) {
            opportunity_save(engine, &input.body, Some(id))
        } else {
            bad_input()
        });
    }
    None
}

fn work_save(engine: &mut Engine, body: &[u8], id: Option<i64>) -> Reply {
    let Some(input) = parse_work(body) else {
        return bad_input();
    };
    let result = match id {
        Some(id) => engine.crm_update_work_item(id, &input).map(|()| id),
        None => engine.crm_create_work_item(&input),
    };
    saved(result, id.is_none())
}

fn opportunity_save(engine: &mut Engine, body: &[u8], id: Option<i64>) -> Reply {
    let Some(input) = parse_opportunity(body) else {
        return bad_input();
    };
    let result = match id {
        Some(id) => engine.crm_update_opportunity(id, &input).map(|()| id),
        None => engine.crm_create_opportunity(&input),
    };
    saved(result, id.is_none())
}

fn work_list(engine: &Engine, query: &str) -> Reply {
    let Some(parameters) = parameters(
        query,
        &["company_id", "kind", "completed", "limit", "after"],
    ) else {
        return bad_input();
    };
    let Some((company, after, limit)) = common_query(&parameters) else {
        return bad_input();
    };
    let kind = match parameters.get("kind").map(String::as_str) {
        None => None,
        Some("task") => Some(WorkKind::Task),
        Some("event") => Some(WorkKind::Event),
        _ => return bad_input(),
    };
    let completed = match parameters.get("completed").map(String::as_str) {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => return bad_input(),
    };
    match engine.crm_work_items(&WorkQuery {
        company,
        kind,
        completed,
        after,
        limit,
    }) {
        Ok(page) => Reply::json(
            200,
            &json!({"items":page.items.iter().map(work_json).collect::<Vec<_>>(),"total":page.total,"next":page.next}),
        ),
        Err(error) => engine_error(&error),
    }
}

fn opportunity_list(engine: &Engine, query: &str) -> Reply {
    let Some(parameters) = parameters(query, &["company_id", "limit", "after"]) else {
        return bad_input();
    };
    let Some((company, after, limit)) = common_query(&parameters) else {
        return bad_input();
    };
    match engine.crm_opportunities(&OpportunityQuery {
        company,
        after,
        limit,
    }) {
        Ok(page) => Reply::json(
            200,
            &json!({"items":page.items.iter().map(opportunity_json).collect::<Vec<_>>(),"total":page.total,"next":page.next}),
        ),
        Err(error) => engine_error(&error),
    }
}

fn parse_work(body: &[u8]) -> Option<WorkItemInput> {
    let o = object(
        body,
        &[
            "company_id",
            "contact_id",
            "kind",
            "subject",
            "due_at",
            "start_at",
            "end_at",
            "completed",
        ],
    )?;
    let company = CompanyId(o.get("company_id")?.as_i64().filter(|v| *v > 0)?);
    let contact = optional_integer(&o, "contact_id")?.map(ContactId);
    if contact.is_some_and(|c| c.0 <= 0) {
        return None;
    }
    let kind = match o.get("kind")?.as_str()? {
        "task" => WorkKind::Task,
        "event" => WorkKind::Event,
        _ => return None,
    };
    let completed = match o.get("completed") {
        None => false,
        Some(value) => value.as_bool()?,
    };
    Some(WorkItemInput {
        company,
        contact,
        kind,
        subject: o.get("subject")?.as_str()?.to_owned(),
        due_at: optional_integer(&o, "due_at")?,
        start_at: optional_integer(&o, "start_at")?,
        end_at: optional_integer(&o, "end_at")?,
        completed,
    })
}

fn parse_opportunity(body: &[u8]) -> Option<OpportunityInput> {
    let o = object(
        body,
        &[
            "company_id",
            "contact_id",
            "name",
            "amount_jpy",
            "stage",
            "close_date",
        ],
    )?;
    let company = CompanyId(o.get("company_id")?.as_i64().filter(|v| *v > 0)?);
    let contact = optional_integer(&o, "contact_id")?.map(ContactId);
    if contact.is_some_and(|c| c.0 <= 0) {
        return None;
    }
    Some(OpportunityInput {
        company,
        contact,
        name: o.get("name")?.as_str()?.to_owned(),
        amount_jpy: o.get("amount_jpy")?.as_i64()?,
        stage: u8::try_from(o.get("stage")?.as_u64()?).ok()?,
        close_date: o.get("close_date")?.as_str()?.to_owned(),
    })
}

#[allow(clippy::option_option)] // malformed / absent / present must remain distinct
fn optional_integer(o: &Map<String, Value>, key: &str) -> Option<Option<i64>> {
    match o.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value.as_i64().map(Some),
    }
}

fn object(body: &[u8], allowed: &[&str]) -> Option<Map<String, Value>> {
    let Value::Object(o) = serde_json::from_slice(body).ok()? else {
        return None;
    };
    if o.keys().any(|key| !allowed.contains(&key.as_str())) {
        return None;
    }
    Some(o)
}

fn positive(value: &str) -> Option<i64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|value| *value > 0)
}

fn parameters(query: &str, allowed: &[&str]) -> Option<HashMap<String, String>> {
    let mut values = HashMap::new();
    if query.is_empty() {
        return Some(values);
    }
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if !allowed.contains(&key) || values.insert(key.to_owned(), value.to_owned()).is_some() {
            return None;
        }
    }
    Some(values)
}

fn common_query(
    parameters: &HashMap<String, String>,
) -> Option<(Option<CompanyId>, Option<i64>, u32)> {
    let company = parameters
        .get("company_id")
        .map(|v| positive(v).map(CompanyId))
        .map_or(Some(None), |v| v.map(Some))?;
    let after = parameters
        .get("after")
        .map(|v| positive(v))
        .map_or(Some(None), |v| v.map(Some))?;
    let limit = match parameters.get("limit") {
        None => 50,
        Some(v) => u32::try_from(positive(v)?).ok().filter(|v| *v <= 200)?,
    };
    Some((company, after, limit))
}

fn work_json(item: &WorkItem) -> Value {
    let input = &item.input;
    json!({"id":item.id,"company_id":input.company.0,"company_name":item.company_name,"contact_id":input.contact.map(|c| c.0),
        "kind":match input.kind { WorkKind::Task=>"task",WorkKind::Event=>"event" },"subject":input.subject,
        "due_at":input.due_at,"start_at":input.start_at,"end_at":input.end_at,"completed":input.completed})
}

fn opportunity_json(item: &Opportunity) -> Value {
    let input = &item.input;
    json!({"id":item.id,"company_id":input.company.0,"company_name":item.company_name,"contact_id":input.contact.map(|c| c.0),
        "name":input.name,"amount_jpy":input.amount_jpy,"stage":input.stage,"close_date":input.close_date})
}

fn saved(result: Result<i64, EngineError>, created: bool) -> Reply {
    match result {
        Ok(id) => Reply::json(if created { 201 } else { 200 }, &json!({"id":id})),
        Err(error) => engine_error(&error),
    }
}

fn bad_input() -> Reply {
    Reply::error(400, "入力内容または検索条件を確認してください。")
}

fn engine_error(error: &EngineError) -> Reply {
    match error {
        EngineError::NotFound => Reply::error(404, "対象が見つかりません。"),
        EngineError::ExcludedCompany => {
            Reply::error(409, "除外中の企業のレコードは更新できません。")
        }
        EngineError::InvalidInput { .. } | EngineError::ForeignContact | EngineError::EmptyName => {
            bad_input()
        }
        _ => Reply::error(500, "データを処理できませんでした。再試行してください。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_json_and_query_inputs() {
        for bad in [
            r#"{"company_id":1,"kind":"task","subject":"x","due_at":1.5}"#,
            r#"{"company_id":0,"kind":"task","subject":"x","due_at":1}"#,
            r#"{"company_id":1,"kind":"task","subject":"x","due_at":1,"extra":1}"#,
            r#"{"company_id":1,"kind":"task","subject":"x","due_at":1,"contact_id":-1}"#,
        ] {
            assert!(parse_work(bad.as_bytes()).is_none(), "{bad}");
        }
        for bad in [
            "after=0",
            "after=-1",
            "after=1&after=2",
            "limit=201",
            "limit=1.5",
            "company_id=%31",
            "unknown=1",
        ] {
            assert!(
                parameters(bad, &["after", "limit", "company_id"])
                    .and_then(|p| common_query(&p))
                    .is_none(),
                "{bad}"
            );
        }
        assert!(parse_opportunity(br#"{"company_id":1,"name":"x","amount_jpy":1.1,"stage":0,"close_date":"2026-10-23"}"#).is_none());
        assert!(
            common_query(
                &parameters(
                    "company_id=1&after=2&limit=200",
                    &["company_id", "after", "limit"]
                )
                .unwrap()
            )
            .is_some()
        );
    }
}
