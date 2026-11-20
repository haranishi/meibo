use super::*;

// REQ 20.1
// REQ 20.2
#[test]
fn absence_reserves_next_weekday_once_and_notifies_only_the_assignee() {
    let (_dir, mut engine, id) = fixture();
    let route = format!("/api/companies/{}/activities", id.0);
    let friday = 1_795_158_000; // 2026-11-20 16:00 JST.
    let monday = 1_795_392_000; // 2026-11-23 09:00 JST.
    let mut payload = json!({"note":"合成テスト・不在","kind":1,"at":friday,"follow_up":"absence","request_key":"synthetic-friday-1"});
    let write = |engine: &mut Engine, body: &Value| {
        dispatch_for_actor(
            engine,
            &input("POST", &route, body),
            PORT,
            Some(10),
            Some("synthetic-scope"),
        )
    };
    let saved = write(&mut engine, &payload);
    assert_eq!(saved.status, 200);
    let row = json_reply(&saved);
    assert_eq!(row["due_at"], monday);
    assert_eq!(json_reply(&write(&mut engine, &payload)), row);
    assert_eq!(engine.crm_due_followups(Some(10), friday).unwrap().total, 0);
    assert_eq!(engine.crm_due_followups(Some(10), monday).unwrap().total, 1);
    assert_eq!(engine.crm_due_followups(Some(11), monday).unwrap().total, 0);
    payload["note"] = json!("合成変更");
    assert_eq!(write(&mut engine, &payload).status, 409);
    assert_eq!(engine.crm_due_followups(None, monday).unwrap().total, 1);
    let work = row["work_id"].as_i64().unwrap();
    engine.crm_set_completion(work, true).unwrap();
    assert_eq!(engine.crm_due_followups(Some(10), monday).unwrap().total, 0);
    assert_eq!(
        engine
            .company_detail(id, &vec![])
            .unwrap()
            .company
            .next_action_at,
        None
    );
}

// REQ 20.1
// REQ 20.2
#[test]
fn explicit_date_and_cancellation_preserve_other_plans_and_refuse_invalid_calls() {
    let (_dir, mut engine, id) = fixture();
    let route = format!("/api/v1/companies/{}/activities", id.0);
    let body = json!({"note":"合成","kind":1,"next_action_at":500,"follow_up":"absence","request_key":"synthetic-call-2020"});
    let response = dispatch_for_actor(
        &mut engine,
        &input("POST", &route, &body),
        PORT,
        Some(7),
        None,
    );
    assert_eq!(response.status, 200);
    let row = json_reply(&response);
    assert_eq!(row["due_at"], 500);
    let activity = row["activity_id"].as_i64().unwrap();
    let work = row["work_id"].as_i64().unwrap();
    let manual = json!({"note":"次の約束","kind":4,"next_action_at":600});
    assert_eq!(
        dispatch(&mut engine, &input("POST", &route, &manual), PORT).status,
        200
    );
    engine.crm_set_completion(work, true).unwrap();
    assert_eq!(
        engine
            .company_detail(id, &vec![])
            .unwrap()
            .company
            .next_action_at,
        Some(600)
    );
    let cancel = format!("/api/companies/{}/activities/{activity}/cancel", id.0);
    assert_eq!(
        dispatch(&mut engine, &input("POST", &cancel, &json!({})), PORT).status,
        200
    );
    assert!(engine.crm_set_completion(work, false).is_err());
    assert_eq!(engine.crm_due_followups(Some(7), 700).unwrap().total, 0);
    for property in [
        json!({"kind":3}),
        json!({"next_action_at":null}),
        json!({"request_key":"short"}),
    ] {
        let mut bad = body.as_object().unwrap().clone();
        bad.extend(property.as_object().unwrap().clone());
        assert_eq!(
            dispatch(
                &mut engine,
                &input("POST", &route, &Value::Object(bad)),
                PORT
            )
            .status,
            400
        );
    }
}
