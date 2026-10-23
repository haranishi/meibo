use super::*;
use meibo_core::types::{CompanyId, CompanyInput, ContactInput, Filters, Status};

const PORT: u16 = 4317;

fn fixture() -> (tempfile::TempDir, Engine, CompanyId) {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &dir.path().join("demo.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let id = engine
        .register_company(&company("合成あおぞら商事"))
        .unwrap();
    (dir, engine, id)
}

fn company(name: &str) -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: None,
        name: name.into(),
        prefecture: 13,
        municipality: Some("13101".into()),
        industry: None,
        phone: Some("0312345678".into()),
        website: None,
        owner: None,
        status: Status::Untouched,
        is_sole_proprietor: false,
        source_id: None,
        source_as_of: None,
    }
}

fn input(method: &str, target: &str, body: &Value) -> Input {
    Input {
        method: method.into(),
        target: target.into(),
        body: body.to_string().into_bytes(),
        headers: vec![
            ("Host".into(), format!("127.0.0.1:{PORT}")),
            ("Origin".into(), format!("http://127.0.0.1:{PORT}")),
            (
                "Content-Type".into(),
                "application/json; charset=utf-8".into(),
            ),
            ("X-Meibo-Local".into(), "1".into()),
        ],
    }
}

fn json_reply(reply: &Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap()
}

// REQ 15.1: 実データDB・存在しないDBはサーバー開始前に拒否する。
#[test]
fn opening_requires_an_existing_synthetic_database() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.db");
    drop(Engine::create(&real, DataOrigin::Real, &OpenOptions::default()).unwrap());
    assert!(open_demo(&real).is_err());
    let missing = dir.path().join("missing.db");
    assert!(open_demo(&missing).is_err());
    assert!(!missing.exists());
    let (_dir, engine, _id) = fixture();
    assert!(open_demo(engine.path().unwrap()).is_ok());
}

// REQ 15.2: DNS rebinding・クロスサイト書込み・不明な形式を境界で拒否する。
#[test]
fn rejects_foreign_hosts_origins_duplicate_headers_and_non_json_posts() {
    let (_dir, mut engine, id) = fixture();
    let target = format!("/api/companies/{}/activities", id.0);
    let valid = input("POST", &target, &json!({"note":"safe"}));
    for (header, value, status) in [
        ("Host", "attacker.example", 403),
        ("Origin", "https://attacker.example", 403),
        ("Origin", "null", 403),
        ("Origin", "http://localhost:4317", 403),
        ("X-Meibo-Local", "0", 403),
        ("Content-Type", "text/plain", 415),
    ] {
        let mut request = input("POST", &target, &json!({"note":"must not save"}));
        request
            .headers
            .iter_mut()
            .find(|(name, _)| name == header)
            .unwrap()
            .1 = value.into();
        assert_eq!(
            dispatch(&mut engine, &request, PORT).status,
            status,
            "{header}"
        );
    }
    for header in ["Host", "Origin", "X-Meibo-Local", "Content-Type"] {
        let mut request = input("POST", &target, &json!({"note":"must not save"}));
        let duplicate = request
            .headers
            .iter()
            .find(|(name, _)| name == header)
            .unwrap()
            .clone();
        request.headers.push(duplicate);
        assert_ne!(
            dispatch(&mut engine, &request, PORT).status,
            200,
            "{header}"
        );
    }
    assert_eq!(engine.company_detail(id, &vec![]).unwrap().activities, []);
    assert_eq!(dispatch(&mut engine, &valid, PORT).status, 200);
}

// REQ 16.3: 保存の原子性と再読込、Keep/Clear、除外解除の禁止。
#[test]
fn activities_persist_and_bad_updates_leave_the_database_unchanged() {
    let (dir, mut engine, id) = fixture();
    let target = format!("/api/companies/{}/activities", id.0);
    let reply = dispatch(
        &mut engine,
        &input(
            "POST",
            &target,
            &json!({"note":"架電結果","status":2,"next_action_at":1_900_000_000}),
        ),
        PORT,
    );
    assert_eq!(reply.status, 200);
    for body in [
        &json!({"note":"bad","status":6}),
        &json!({"note":false}),
        &json!({"note":"bad","next_action_at":"tomorrow"}),
        &json!({"note":"bad","next_action_at":-1}),
        &json!({"note":"bad","kind":9}),
        &json!({"status":1}),
        &json!(["not an object"]),
    ] {
        assert_eq!(
            dispatch(&mut engine, &input("POST", &target, body), PORT).status,
            400
        );
    }
    let mut too_large = input("POST", &target, &json!({"note":"safe"}));
    too_large.body = vec![b' '; MAX_BODY + 1];
    assert_eq!(dispatch(&mut engine, &too_large, PORT).status, 413);
    let reply = dispatch(
        &mut engine,
        &input("POST", &target, &json!({"note":"再連絡"})),
        PORT,
    );
    assert_eq!(reply.status, 200);
    drop(engine);
    let mut reopened = open_demo(&dir.path().join("demo.db")).unwrap();
    let detail = reopened.company_detail(id, &vec![]).unwrap();
    assert_eq!(detail.company.status, Status::Negotiating);
    assert_eq!(detail.company.next_action_at, Some(1_900_000_000));
    assert_eq!(detail.activities.len(), 2);
    assert_eq!(
        detail.activities[0].kind,
        meibo_core::types::ActivityKind::Call
    );
    assert_eq!(
        dispatch(
            &mut reopened,
            &input(
                "POST",
                &target,
                &json!({"note":"対応日解除","next_action_at":null})
            ),
            PORT
        )
        .status,
        200
    );
    assert_eq!(
        reopened
            .company_detail(id, &vec![])
            .unwrap()
            .company
            .next_action_at,
        None
    );
    assert_eq!(
        dispatch(
            &mut reopened,
            &input("POST", &target, &json!({"note":"除外","status":5})),
            PORT
        )
        .status,
        200
    );
    assert_eq!(
        dispatch(
            &mut reopened,
            &input("POST", &target, &json!({"note":"不正な解除","status":0})),
            PORT
        )
        .status,
        409
    );
    assert_eq!(
        reopened
            .company_detail(id, &vec![])
            .unwrap()
            .activities
            .len(),
        4
    );
    let report = reopened.verify().unwrap();
    assert!(report.integrity_ok && report.counts_match && report.folds_match);
}

// REQ 16.1/16.2: ページと件数、既定マスク、消去済み詳細をHTTPでも維持する。
#[test]
fn listing_search_detail_and_export_keep_core_privacy_rules() {
    let (_dir, mut engine, id) = fixture();
    engine.register_company(&company("合成みどり商事")).unwrap();
    let mut personal = company("非表示の個人事業者");
    personal.is_sole_proprietor = true;
    let personal_id = engine.register_company(&personal).unwrap();
    engine
        .add_contact(&ContactInput {
            company: id,
            name: "非表示の担当者".into(),
            phone: Some("0311111111".into()),
            email: Some("private@example.test".into()),
        })
        .unwrap();
    let response = dispatch(
        &mut engine,
        &input("GET", "/api/companies?limit=1", &Value::Null),
        PORT,
    );
    let first = json_reply(&response);
    assert_eq!(first["count"], 3);
    assert_eq!(first["rows"].as_array().unwrap().len(), 1);
    let target = format!(
        "/api/companies?limit=1&after={}",
        first["next"].as_str().unwrap()
    );
    let second = json_reply(&dispatch(
        &mut engine,
        &input("GET", &target, &Value::Null),
        PORT,
    ));
    assert_ne!(first["rows"][0]["id"], second["rows"][0]["id"]);
    assert!(second["count"].is_null());
    let search = json_reply(&dispatch(
        &mut engine,
        &input(
            "GET",
            "/api/companies?q=%E3%81%BF%E3%81%A9%E3%82%8A",
            &Value::Null,
        ),
        PORT,
    ));
    assert_eq!(search["count"], 1);
    assert_eq!(search["rows"][0]["name"], "合成みどり商事");
    let personal_detail = json_reply(&dispatch(
        &mut engine,
        &input(
            "GET",
            &format!("/api/companies/{}", personal_id.0),
            &Value::Null,
        ),
        PORT,
    ));
    assert!(personal_detail["company"]["name"].is_null());
    assert!(personal_detail["company"]["phone"].is_null());
    let detail_reply = dispatch(
        &mut engine,
        &input("GET", &format!("/api/companies/{}", id.0), &Value::Null),
        PORT,
    );
    let detail = json_reply(&detail_reply);
    assert_eq!(detail["company"]["municipality"], "13101");
    assert!(detail["contacts"][0]["name"].is_null());
    assert!(!String::from_utf8_lossy(&detail_reply.body).contains("private@example"));
    let csv = dispatch(
        &mut engine,
        &input("GET", "/api/export", &Value::Null),
        PORT,
    );
    assert_eq!(csv.status, 200);
    assert!(csv.attachment);
    assert!(!String::from_utf8_lossy(&csv.body).contains("非表示"));
    engine.erase_company(personal_id).unwrap();
    assert_eq!(
        dispatch(
            &mut engine,
            &input(
                "GET",
                &format!("/api/companies/{}", personal_id.0),
                &Value::Null
            ),
            PORT
        )
        .status,
        404
    );
    assert_eq!(
        dispatch(
            &mut engine,
            &input(
                "POST",
                &format!("/api/companies/{}/activities", personal_id.0),
                &json!({"note":"refused"})
            ),
            PORT
        )
        .status,
        404
    );
}

#[test]
fn malformed_routes_and_search_do_not_disclose_input_values() {
    let (_dir, mut engine, _) = fixture();
    for target in [
        "/api/companies?after=DO_NOT_REFLECT_SECRET",
        "/api/companies?q=%FF",
        "/api/companies?q=%",
        "/api/companies?limit=999",
        "/api/companies?limit=1&limit=2",
        "/api/companies?unknown=DO_NOT_REFLECT_SECRET",
    ] {
        let reply = dispatch(&mut engine, &input("GET", target, &Value::Null), PORT);
        assert_eq!(reply.status, 400, "{target}");
        assert!(!String::from_utf8_lossy(&reply.body).contains("DO_NOT_REFLECT_SECRET"));
    }
    for target in [
        "/missing",
        "/api/companies/0",
        "/api/companies/-1",
        "/api/companies/1/extra",
        "/../Cargo.toml",
    ] {
        assert_eq!(
            dispatch(&mut engine, &input("GET", target, &Value::Null), PORT).status,
            404
        );
    }
    for target in [
        "http://attacker.example/health",
        "//attacker.example/health",
    ] {
        assert_eq!(
            dispatch(&mut engine, &input("GET", target, &Value::Null), PORT).status,
            400
        );
    }
    assert_eq!(
        dispatch(
            &mut engine,
            &input("DELETE", "/api/companies/1", &Value::Null),
            PORT
        )
        .status,
        405
    );
}

fn workspace_fixture() -> (tempfile::TempDir, Engine, Vec<CompanyId>) {
    let (dir, mut engine, first) = fixture();
    let mut osaka = company("合成大阪商談");
    osaka.prefecture = 27;
    osaka.municipality = None;
    osaka.status = Status::Negotiating;
    osaka.phone = None;
    let osaka = engine.register_company(&osaka).unwrap();
    let mut tokyo = company("合成東京商談");
    tokyo.status = Status::Negotiating;
    let tokyo = engine.register_company(&tokyo).unwrap();
    let mut active = company("合成対応中");
    active.status = Status::InProgress;
    let active = engine.register_company(&active).unwrap();
    let mut excluded = company("合成除外");
    excluded.status = Status::Excluded;
    let excluded = engine.register_company(&excluded).unwrap();
    for (id, at) in [(osaka, 1), (tokyo, i64::MAX - 1), (excluded, 2)] {
        assert_eq!(
            dispatch(
                &mut engine,
                &input(
                    "POST",
                    &format!("/api/companies/{}/activities", id.0),
                    &json!({"note":"合成予定","next_action_at":at}),
                ),
                PORT
            )
            .status,
            200
        );
    }
    (dir, engine, vec![first, osaka, tokyo, active, excluded])
}

// REQ 18.1: ホームの集計は状態別件数と予定を実際の保存内容から求める。
#[test]
fn workspace_summary_counts_statuses_and_excludes_suppressed_schedules() {
    let (_dir, mut engine, _) = workspace_fixture();
    let reply = dispatch(
        &mut engine,
        &input("GET", "/api/summary", &Value::Null),
        PORT,
    );
    assert_eq!(reply.status, 200);
    let data = json_reply(&reply);
    assert_eq!(data["total"], 5);
    assert_eq!(data["active"], 4);
    assert_eq!(data["with_phone"], 3);
    assert_eq!(data["status_counts"], json!([1, 1, 2, 0, 0, 1]));
    assert_eq!(data["scheduled"], 2);
    assert_eq!(data["due"], 1);
}

// REQ 18.2: ビュー・条件・カーソルをサーバーで適用し、予定を日時順で返す。
#[test]
fn workspace_views_filters_and_cursors_match_their_counts() {
    let (_dir, mut engine, ids) = workspace_fixture();
    for (query, expected) in [
        ("view=all", 4usize),
        ("view=untouched", 1),
        ("view=negotiating", 2),
        ("view=has_phone", 3),
        ("view=scheduled", 2),
        ("view=due", 1),
        ("status=2&prefecture=13", 1),
        ("has_phone=false", 1),
        ("view=negotiating&status=1", 1),
        ("view=scheduled&status=1", 0),
    ] {
        let reply = dispatch(
            &mut engine,
            &input("GET", &format!("/api/companies?{query}"), &Value::Null),
            PORT,
        );
        assert_eq!(reply.status, 200, "{query}");
        let data = json_reply(&reply);
        assert_eq!(data["count"], expected, "{query}");
        assert_eq!(data["rows"].as_array().unwrap().len(), expected, "{query}");
    }
    let first = json_reply(&dispatch(
        &mut engine,
        &input("GET", "/api/companies?view=scheduled&limit=1", &Value::Null),
        PORT,
    ));
    assert_eq!(first["rows"][0]["id"], ids[1].0);
    let next = format!(
        "/api/companies?view=scheduled&limit=1&after={}",
        first["next"].as_str().unwrap()
    );
    let second = json_reply(&dispatch(
        &mut engine,
        &input("GET", &next, &Value::Null),
        PORT,
    ));
    assert_eq!(second["rows"][0]["id"], ids[2].0);
    assert!(second["count"].is_null());
    assert!(second["next"].is_null());
}

// REQ 18.3: 検索とビューの未対応併用、不正/重複の条件を無言で無視しない。
#[test]
fn workspace_search_filter_combinations_and_bad_parameters_are_rejected() {
    let (_dir, mut engine, _) = fixture();
    for query in [
        "q=SECRET&view=negotiating",
        "q=SECRET&status=0",
        "q=SECRET&prefecture=13",
        "q=SECRET&has_phone=true",
        "view=unknown",
        "view=all&view=all",
        "status=5",
        "status=-1",
        "status=word",
        "prefecture=0",
        "prefecture=48",
        "has_phone=1",
        "has_phone=TRUE",
        "has_phone=true&has_phone=false",
    ] {
        let reply = dispatch(
            &mut engine,
            &input("GET", &format!("/api/companies?{query}"), &Value::Null),
            PORT,
        );
        assert_eq!(reply.status, 400, "{query}");
        assert!(!String::from_utf8_lossy(&reply.body).contains("SECRET"));
    }
    assert_eq!(
        dispatch(
            &mut engine,
            &input("GET", "/api/companies?q=all&view=all", &Value::Null,),
            PORT
        )
        .status,
        200
    );
}

// REQ 18.6: 合成取引先を追加して再openし、重複・不正入力は既存行を変えない。
#[test]
fn creating_accounts_preserves_data_and_reports_duplicates_and_invalid_fields() {
    let (dir, mut engine, existing) = fixture();
    let body = json!({"name":" 合成新規取引先 ","prefecture":27,
        "corporate_number":"6600000000000","phone":"０３-１２３４","website":"https://synthetic.invalid"});
    let reply = dispatch(&mut engine, &input("POST", "/api/companies", &body), PORT);
    assert_eq!(reply.status, 201);
    let id = CompanyId(json_reply(&reply)["id"].as_i64().unwrap());
    assert_ne!(id, existing);
    drop(engine);
    let mut engine = open_demo(&dir.path().join("demo.db")).unwrap();
    let saved = engine.company_detail(id, &vec![]).unwrap().company;
    assert_eq!(saved.name.as_deref(), Some("合成新規取引先"));
    assert_eq!(saved.origin, DataOrigin::Synthetic);
    assert_eq!(saved.status, Status::Untouched);
    assert!(!saved.is_sole_proprietor);
    assert_eq!(saved.phone.as_deref(), Some("03-1234"));
    assert_eq!(saved.prefecture, 27);
    assert_eq!(saved.source_id.as_deref(), Some("manual-demo"));
    assert_eq!(
        dispatch(&mut engine, &input("POST", "/api/companies", &body), PORT).status,
        409
    );
    for body in [
        json!({"prefecture":13}),
        json!({"name":"missing prefecture"}),
        json!({"name":" ","prefecture":13}),
        json!({"name":"bad","prefecture":0}),
        json!({"name":"bad","prefecture":48}),
        json!({"name":"bad","prefecture":"13"}),
        json!({"name":"bad","prefecture":13,"corporate_number":"123"}),
        json!({"name":"bad","prefecture":13,"phone":false}),
        json!({"name":"bad","prefecture":13,"website":42}),
        json!({"name":"bad","prefecture":13,"origin":"real"}),
        json!({"name":"bad","prefecture":13,"status":3}),
        json!({"name":"bad\u{0000}","prefecture":13}),
        json!(["not an object"]),
    ] {
        assert_eq!(
            dispatch(&mut engine, &input("POST", "/api/companies", &body), PORT).status,
            400
        );
    }
    assert_eq!(engine.count(&Filters::default()).unwrap().n, 2);
    assert_eq!(
        engine
            .company_detail(existing, &vec![])
            .unwrap()
            .company
            .name
            .as_deref(),
        Some("合成あおぞら商事")
    );
    let verification = engine.verify().unwrap();
    assert!(verification.integrity_ok && verification.counts_match && verification.folds_match);
}

// REQ 18.6: 新規登録も既存のOrigin・容量制限を迂回しない。
#[test]
fn account_creation_keeps_the_same_origin_and_body_boundaries() {
    let (_dir, mut engine, _) = fixture();
    let body = json!({"name":"合成未保存","prefecture":13});
    let mut foreign = input("POST", "/api/companies", &body);
    foreign
        .headers
        .iter_mut()
        .find(|(name, _)| name == "Origin")
        .unwrap()
        .1 = "https://attacker.invalid".into();
    assert_eq!(dispatch(&mut engine, &foreign, PORT).status, 403);
    let mut large = input("POST", "/api/companies", &body);
    large.body = vec![b' '; MAX_BODY + 1];
    assert_eq!(dispatch(&mut engine, &large, PORT).status, 413);
    assert_eq!(engine.count(&Filters::default()).unwrap().n, 1);
}
