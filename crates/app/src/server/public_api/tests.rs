use super::*;
use meibo_core::{
    OpenOptions,
    types::{CompanyId, CompanyInput, ContactInput, DataOrigin, Status},
};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Engine, Vec<CompanyId>) {
    let directory = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &directory.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let ids = ["合成あおぞら", "合成ひかり", "個人事業の秘密名"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            engine
                .register_company(&CompanyInput {
                    origin: DataOrigin::Synthetic,
                    corporate_number: None,
                    name: name.into(),
                    prefecture: 13,
                    municipality: Some("13101".into()),
                    industry: None,
                    phone: Some("000-0000-0000".into()),
                    website: None,
                    owner: None,
                    status: Status::Untouched,
                    is_sole_proprietor: index == 2,
                    source_id: Some(
                        if index == 0 {
                            "nta"
                        } else {
                            "synthetic-source"
                        }
                        .into(),
                    ),
                    source_as_of: Some("2026-10-26".into()),
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    engine
        .add_contact(&ContactInput {
            company: ids[0],
            name: "担当者の秘密名".into(),
            phone: Some("contact-secret-phone".into()),
            email: Some("synthetic@example.invalid".into()),
        })
        .unwrap();
    (directory, engine, ids)
}

fn input(method: &str, target: &str, body: &Value) -> Input {
    Input {
        method: method.into(),
        target: target.into(),
        headers: vec![
            ("Host".into(), "127.0.0.1:4317".into()),
            ("Origin".into(), "http://127.0.0.1:4317".into()),
            ("X-Meibo-Local".into(), "1".into()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body: body.to_string().into_bytes(),
    }
}

fn request(engine: &mut Engine, method: &str, target: &str, body: &Value) -> Reply {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    dispatch(engine, &input(method, target, body), path, query).expect("v1 route")
}

fn value(reply: &Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap()
}

#[test]
fn public_data_source_date_and_notices_are_shared_with_tools() {
    let (_directory, mut engine, ids) = fixture();
    let target = format!("/api/v1/companies/{}", ids[0].0);
    let http = value(&request(&mut engine, "GET", &target, &Value::Null));
    let (failed, tool) =
        call_tool(&mut engine, "get_company", &json!({"id":ids[0].0}), false).unwrap();
    assert!(!failed);
    assert_eq!(http, tool);
    assert_eq!(http["company"]["source_id"], "nta");
    assert_eq!(http["company"]["source_as_of"], "2026-10-26");
    assert_ne!(http["company"]["attribution"].as_str().unwrap(), "");
    assert_ne!(http["company"]["modification_notice"].as_str().unwrap(), "");
}

#[test]
fn v1_list_uses_existing_pagination_and_search_rules() {
    let (_directory, mut engine, ids) = fixture();
    let first = request(
        &mut engine,
        "GET",
        "/api/v1/companies?limit=1",
        &Value::Null,
    );
    assert_eq!(first.status, 200);
    let first = value(&first);
    assert_eq!(first["rows"].as_array().unwrap().len(), 1);
    let next = first["next"].as_str().unwrap();
    let second = value(&request(
        &mut engine,
        "GET",
        &format!("/api/v1/companies?limit=1&after={next}"),
        &Value::Null,
    ));
    assert_ne!(first["rows"][0]["id"], second["rows"][0]["id"]);
    let searched = request(
        &mut engine,
        "GET",
        "/api/v1/companies?q=%E3%81%82%E3%81%8A%E3%81%9E%E3%82%89",
        &Value::Null,
    );
    assert_eq!(searched.status, 200);
    assert_eq!(value(&searched)["rows"][0]["id"], ids[0].0);
    for query in [
        "limit=0",
        "limit=201",
        "limit=1&limit=2",
        "unknown=true",
        "q=%GG",
    ] {
        assert_eq!(
            request(
                &mut engine,
                "GET",
                &format!("/api/v1/companies?{query}"),
                &Value::Null
            )
            .status,
            400,
            "{query}"
        );
    }
}

#[test]
fn v1_details_hide_contact_and_sole_proprietor_data_by_default() {
    let (_directory, mut engine, ids) = fixture();
    let detail = request(
        &mut engine,
        "GET",
        &format!("/api/v1/companies/{}", ids[0].0),
        &Value::Null,
    );
    assert_eq!(detail.status, 200);
    let body = String::from_utf8(detail.body).unwrap();
    assert!(!body.contains("担当者の秘密名"));
    assert!(!body.contains("contact-secret-phone"));
    let personal = request(
        &mut engine,
        "GET",
        &format!("/api/v1/companies/{}", ids[2].0),
        &Value::Null,
    );
    assert!(
        !String::from_utf8(personal.body)
            .unwrap()
            .contains("個人事業の秘密名")
    );
    let explicit = request(
        &mut engine,
        "GET",
        &format!("/api/v1/companies/{}?include_contacts=1", ids[0].0),
        &Value::Null,
    );
    assert!(
        String::from_utf8(explicit.body)
            .unwrap()
            .contains("担当者の秘密名")
    );
}

#[test]
fn v1_activity_persists_and_invalid_inputs_do_not_add_records() {
    let (directory, mut engine, ids) = fixture();
    let path = format!("/api/v1/companies/{}/activities", ids[0].0);
    for body in [
        &json!({"note":false}),
        &json!({"note":"reject", "unknown":1}),
        &json!({"note":"reject", "status":99}),
    ] {
        assert_eq!(request(&mut engine, "POST", &path, body).status, 400);
    }
    assert_eq!(
        engine
            .company_detail(ids[0], &vec![])
            .unwrap()
            .activities
            .len(),
        0
    );
    assert_eq!(
        request(
            &mut engine,
            "POST",
            &path,
            &json!({"note":"合成の架電結果", "next_action_at":1_900_000_000})
        )
        .status,
        200
    );
    drop(engine);
    let engine = Engine::open(
        &directory.path().join("synthetic.db"),
        &OpenOptions::default(),
    )
    .unwrap();
    let detail = engine.company_detail(ids[0], &vec![]).unwrap();
    assert_eq!(detail.activities.len(), 1);
    assert_eq!(detail.company.next_action_at, Some(1_900_000_000));
}

#[test]
fn v1_method_path_and_outer_http_boundary_remain_enforced() {
    let (_directory, mut engine, ids) = fixture();
    assert!(
        dispatch(
            &mut engine,
            &input("GET", "/api/companies", &Value::Null),
            "/api/companies",
            ""
        )
        .is_none()
    );
    assert_eq!(
        request(&mut engine, "POST", "/api/v1/companies", &json!({})).status,
        405
    );
    for path in [
        "/api/v1/companies/0",
        "/api/v1/companies/-1",
        "/api/v1/companies/1/unknown",
        "/api/v1/unknown",
    ] {
        assert_eq!(request(&mut engine, "GET", path, &Value::Null).status, 404);
    }
    let target = format!("/api/v1/companies/{}/activities", ids[0].0);
    let mut malicious = input("POST", &target, &json!({"note":"must not save"}));
    malicious.headers[1].1 = "https://untrusted.invalid".into();
    assert_eq!(
        super::super::dispatch(&mut engine, &malicious, 4317).status,
        403
    );
    assert_eq!(
        engine.company_detail(ids[0], &vec![]).unwrap().activities,
        []
    );
}

#[test]
fn tool_arguments_are_typed_and_read_only_mode_cannot_write() {
    let (_directory, mut engine, ids) = fixture();
    let (failed, list) =
        call_tool(&mut engine, "list_companies", &json!({"limit":1}), false).unwrap();
    assert!(!failed);
    assert_eq!(list["rows"].as_array().unwrap().len(), 1);
    for arguments in [
        &json!({"limit":"1"}),
        &json!({"limit":201}),
        &json!({"owner":1}),
        &json!({"has_phone":"true"}),
    ] {
        assert!(
            call_tool(&mut engine, "list_companies", arguments, false)
                .unwrap()
                .0
        );
    }
    assert!(
        call_tool(
            &mut engine,
            "record_activity",
            &json!({"company_id":ids[0].0,"note":"must not save"}),
            false
        )
        .is_err()
    );
    assert_eq!(
        engine.company_detail(ids[0], &vec![]).unwrap().activities,
        []
    );
    let (failed, _) = call_tool(
        &mut engine,
        "record_activity",
        &json!({"company_id":ids[0].0,"note":"合成の記録"}),
        true,
    )
    .unwrap();
    assert!(!failed);
    assert_eq!(
        engine
            .company_detail(ids[0], &vec![])
            .unwrap()
            .activities
            .len(),
        1
    );
}
