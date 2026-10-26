//! 独自項目を明示して操作するJSON入口。HTTP認証・同一オリジン検証は呼出元が行う。
use super::{Input, Reply};
use meibo_core::{
    Engine,
    custom_fields::{
        CustomField, CustomFieldInput, CustomFieldKind, CustomFieldSearch, CustomFieldValue,
    },
    types::{CompanyId, EngineError},
};
use serde_json::{Map, Value, json};

pub(super) fn dispatch(
    engine: &mut Engine,
    input: &Input,
    path: &str,
    query: &str,
) -> Option<Reply> {
    if path == "/api/custom-fields" {
        return Some(if query.is_empty() {
            match input.method.as_str() {
                "GET" => match engine.custom_fields() {
                    Ok(fields) => Reply::json(
                        200,
                        &json!({"fields":fields.iter().map(field_json).collect::<Vec<_>>()}),
                    ),
                    Err(error) => engine_error(&error),
                },
                "POST" => create(engine, &input.body),
                _ => bad_input(),
            }
        } else {
            bad_input()
        });
    }
    if let Some(rest) = path.strip_prefix("/api/custom-fields/") {
        return Some(if input.method != "POST" || !query.is_empty() {
            bad_input()
        } else {
            match rest.split_once('/') {
                Some((id, "delete")) => match (positive(id), object(&input.body, &[])) {
                    (Some(id), Some(_)) => saved(engine.delete_custom_field(id)),
                    _ => bad_input(),
                },
                Some((id, "search")) => match positive(id) {
                    Some(id) => search(engine, id, &input.body),
                    None => bad_input(),
                },
                _ => bad_input(),
            }
        });
    }
    if let Some(rest) = path
        .strip_prefix("/api/companies/")
        .and_then(|value| value.strip_suffix("/custom-fields"))
    {
        return Some(if !query.is_empty() {
            bad_input()
        } else if let Some(id) = positive(rest) {
            match input.method.as_str() {
                "GET" => company_values(engine, CompanyId(id)),
                "POST" => save_value(engine, CompanyId(id), &input.body),
                _ => bad_input(),
            }
        } else {
            bad_input()
        });
    }
    None
}

fn create(engine: &mut Engine, body: &[u8]) -> Reply {
    let Some(input) = object(body, &["name", "kind", "indexed"]).and_then(|o| {
        Some(CustomFieldInput {
            name: o.get("name")?.as_str()?.to_owned(),
            kind: match o.get("kind")?.as_str()? {
                "text" => CustomFieldKind::Text,
                "number" => CustomFieldKind::Number,
                "boolean" => CustomFieldKind::Boolean,
                _ => return None,
            },
            indexed: o.get("indexed")?.as_bool()?,
        })
    }) else {
        return bad_input();
    };
    match engine.create_custom_field(&input) {
        Ok(id) => Reply::json(201, &json!({"id":id})),
        Err(error) => engine_error(&error),
    }
}

fn company_values(engine: &Engine, company: CompanyId) -> Reply {
    let result = engine
        .custom_values(company)
        .and_then(|values| Ok((engine.custom_fields()?, values)));
    match result {
        Ok((fields, values)) => Reply::json(
            200,
            &json!({"fields":fields.iter().map(field_json).collect::<Vec<_>>(),"values":values.iter().map(|item|json!({"field_id":item.field_id,"value":value_json(&item.value)})).collect::<Vec<_>>()}),
        ),
        Err(error) => engine_error(&error),
    }
}

fn save_value(engine: &mut Engine, company: CompanyId, body: &[u8]) -> Reply {
    let Some(object) = object(body, &["field_id", "value"]) else {
        return bad_input();
    };
    let Some(field) = object
        .get("field_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
    else {
        return bad_input();
    };
    match object.get("value") {
        Some(Value::Null) => saved(engine.clear_custom_value(company, field)),
        Some(value) => match typed_value(value) {
            Some(value) => saved(engine.set_custom_value(company, field, &value)),
            None => bad_input(),
        },
        None => bad_input(),
    }
}

fn search(engine: &Engine, field_id: i64, body: &[u8]) -> Reply {
    let Some(query) = object(body, &["value", "after", "limit"]).and_then(|o| {
        let value = typed_value(o.get("value")?)?;
        let after = match o.get("after") {
            None | Some(Value::Null) => None,
            Some(value) => Some(CompanyId(value.as_i64().filter(|id| *id > 0)?)),
        };
        let limit = match o.get("limit") {
            None => 50,
            Some(value) => u32::try_from(value.as_u64()?)
                .ok()
                .filter(|limit| (1..=200).contains(limit))?,
        };
        Some(CustomFieldSearch {
            field_id,
            value,
            after,
            limit,
        })
    }) else {
        return bad_input();
    };
    match engine.search_custom_field(&query) {
        Ok(page) => Reply::json(
            200,
            &json!({"company_ids":page.company_ids.iter().map(|id|id.0).collect::<Vec<_>>(),"next":page.next.map(|id|id.0),"indexed":page.indexed}),
        ),
        Err(error) => engine_error(&error),
    }
}

fn positive(value: &str) -> Option<i64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|id| *id > 0)
}

fn object(body: &[u8], allowed: &[&str]) -> Option<Map<String, Value>> {
    let Value::Object(object) = serde_json::from_slice(body).ok()? else {
        return None;
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return None;
    }
    Some(object)
}

fn typed_value(value: &Value) -> Option<CustomFieldValue> {
    match value {
        Value::String(text) => Some(CustomFieldValue::Text(text.clone())),
        Value::Bool(boolean) => Some(CustomFieldValue::Boolean(*boolean)),
        Value::Number(number) => number
            .as_f64()
            .filter(|number| number.is_finite())
            .map(CustomFieldValue::Number),
        _ => None,
    }
}

fn field_json(field: &CustomField) -> Value {
    json!({"id":field.id,"name":field.name,"kind":match field.kind{CustomFieldKind::Text=>"text",CustomFieldKind::Number=>"number",CustomFieldKind::Boolean=>"boolean"},"indexed":field.indexed})
}

fn value_json(value: &CustomFieldValue) -> Value {
    match value {
        CustomFieldValue::Text(text) => json!(text),
        CustomFieldValue::Number(number) => json!(number),
        CustomFieldValue::Boolean(boolean) => json!(boolean),
    }
}

fn saved(result: Result<(), EngineError>) -> Reply {
    match result {
        Ok(()) => Reply::json(200, &json!({"ok":true})),
        Err(error) => engine_error(&error),
    }
}

fn bad_input() -> Reply {
    Reply::error(400, "独自項目の入力内容または検索条件を確認してください。")
}

fn engine_error(error: &EngineError) -> Reply {
    match error {
        EngineError::NotFound => Reply::error(404, "取引先または独自項目が見つかりません。"),
        EngineError::InvalidInput { field, reason }
            if field == "name" && reason == "name already exists" =>
        {
            Reply::error(400, "同じ名前の独自項目が存在します。")
        }
        EngineError::InvalidInput { field, reason }
            if field == "custom_fields" && reason == "definition limit exceeded" =>
        {
            Reply::error(
                400,
                "独自項目は100件までです。不要な項目を削除してください。",
            )
        }
        EngineError::InvalidInput { .. } => bad_input(),
        EngineError::InsufficientSpace => Reply::error(
            507,
            "保存先の空き容量が足りません。入力を保持して再試行してください。",
        ),
        _ => Reply::error(
            500,
            "独自項目を処理できませんでした。入力を保持して再試行してください。",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meibo_core::{
        OpenOptions,
        types::{CompanyInput, DataOrigin, Status},
    };
    use serde_json::{Value, json};

    fn fixture() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::create(
            &dir.path().join("fields-http.db"),
            DataOrigin::Synthetic,
            &OpenOptions::default(),
        )
        .unwrap();
        engine
            .register_company(&CompanyInput {
                origin: DataOrigin::Synthetic,
                corporate_number: None,
                name: "合成取引先".into(),
                prefecture: 13,
                municipality: None,
                industry: None,
                phone: None,
                website: None,
                owner: None,
                status: Status::Untouched,
                is_sole_proprietor: false,
                source_id: None,
                source_as_of: None,
            })
            .unwrap();
        (dir, engine)
    }
    fn call(engine: &mut Engine, method: &str, path: &str, body: &Value) -> Reply {
        let input = Input {
            method: method.into(),
            target: path.into(),
            headers: Vec::new(),
            body: body.to_string().into_bytes(),
        };
        dispatch(engine, &input, path, "").expect("custom field route must exist")
    }
    fn decode(reply: Reply) -> Value {
        serde_json::from_reader(std::io::Cursor::new(reply.body)).unwrap()
    }
    fn create(engine: &mut Engine, kind: &str) -> i64 {
        let reply = call(
            engine,
            "POST",
            "/api/custom-fields",
            &json!({"name":format!("合成{kind}"),"kind":kind,"indexed":true}),
        );
        assert_eq!(reply.status, 201);
        decode(reply)["id"].as_i64().unwrap()
    }

    #[test]
    fn explicit_routes_create_store_read_clear_and_delete_values() {
        let (_dir, mut engine) = fixture();
        let id = create(&mut engine, "text");
        let path = "/api/companies/1/custom-fields";
        assert_eq!(
            call(
                &mut engine,
                "POST",
                path,
                &json!({"field_id":id,"value":"内部だけの合成メモ"})
            )
            .status,
            200
        );
        let result = decode(call(&mut engine, "GET", path, &Value::Null));
        assert_eq!(result["fields"][0]["kind"], "text");
        assert_eq!(result["values"][0]["value"], "内部だけの合成メモ");
        assert_eq!(
            call(
                &mut engine,
                "POST",
                path,
                &json!({"field_id":id,"value":null})
            )
            .status,
            200
        );
        assert_eq!(
            decode(call(&mut engine, "GET", path, &Value::Null))["values"],
            json!([])
        );
        assert_eq!(
            call(
                &mut engine,
                "POST",
                &format!("/api/custom-fields/{id}/delete"),
                &json!({})
            )
            .status,
            200
        );
        assert_eq!(
            decode(call(&mut engine, "GET", "/api/custom-fields", &Value::Null))["fields"],
            json!([])
        );
    }

    #[test]
    fn malformed_inputs_fail_without_implicit_coercion_or_extra_keys() {
        let (_dir, mut engine) = fixture();
        for bad in [
            json!({"name":"x","kind":"date","indexed":true}),
            json!({"name":"x","kind":"text","indexed":"true"}),
            json!({"name":"x","kind":"text","indexed":false,"extra":1}),
            json!({"name":"x","kind":"text"}),
        ] {
            assert_eq!(
                call(&mut engine, "POST", "/api/custom-fields", &bad).status,
                400
            );
        }
        let id = create(&mut engine, "number");
        for bad in [
            json!({"field_id":id,"value":"12"}),
            json!({"field_id":1.5,"value":12}),
            json!({"field_id":id,"value":[1]}),
            json!({"field_id":id,"value":12,"extra":1}),
            json!({"field_id":id}),
        ] {
            assert_eq!(
                call(&mut engine, "POST", "/api/companies/1/custom-fields", &bad).status,
                400
            );
        }
        assert_eq!(
            decode(call(
                &mut engine,
                "GET",
                "/api/companies/1/custom-fields",
                &Value::Null
            ))["values"],
            json!([])
        );
        assert_eq!(
            call(
                &mut engine,
                "POST",
                "/api/companies/999/custom-fields",
                &json!({"field_id":id,"value":1})
            )
            .status,
            404
        );
    }

    #[test]
    fn dedicated_search_requires_the_typed_value_and_bounded_cursor() {
        let (_dir, mut engine) = fixture();
        let id = create(&mut engine, "boolean");
        call(
            &mut engine,
            "POST",
            "/api/companies/1/custom-fields",
            &json!({"field_id":id,"value":false}),
        );
        let path = format!("/api/custom-fields/{id}/search");
        let reply = call(
            &mut engine,
            "POST",
            &path,
            &json!({"value":false,"limit":1}),
        );
        assert_eq!(reply.status, 200);
        assert_eq!(
            decode(reply),
            json!({"company_ids":[1],"next":null,"indexed":true})
        );
        for bad in [
            json!({"value":"false"}),
            json!({"value":false,"limit":201}),
            json!({"value":false,"limit":0}),
            json!({"value":false,"after":-1}),
            json!({"value":false,"extra":1}),
        ] {
            assert_eq!(call(&mut engine, "POST", &path, &bad).status, 400);
        }
    }

    #[test]
    fn routes_reject_unknown_query_and_invalid_identifiers() {
        let (_dir, mut engine) = fixture();
        for path in [
            "/api/custom-fields/0/delete",
            "/api/custom-fields/-1/search",
            "/api/companies/1.0/custom-fields",
            "/api/companies/0/custom-fields",
        ] {
            assert_eq!(call(&mut engine, "POST", path, &json!({})).status, 400);
        }
        let input = Input {
            method: "GET".into(),
            target: "/api/custom-fields?extra=1".into(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert_eq!(
            dispatch(&mut engine, &input, "/api/custom-fields", "extra=1")
                .unwrap()
                .status,
            400
        );
        assert!(dispatch(&mut engine, &input, "/api/unrelated", "").is_none());
    }
}
