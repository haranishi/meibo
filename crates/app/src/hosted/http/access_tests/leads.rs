use super::*;

struct Operator {
    token: String,
    session: Session,
}

impl Operator {
    fn signed_in(app: &mut Hosted, email: &str, tenant: &str) -> Self {
        Self {
            token: jwt(email),
            session: login(app, email, tenant),
        }
    }

    fn send(&self, app: &mut Hosted, path: &str, method: &str, body: &Value) -> Reply {
        app.dispatch(
            &request(path, method, Some(&self.token), Some(&self.session), body),
            proof::AT,
        )
        .reply
    }
}

fn public(app: &mut Hosted, path: &str, method: &str, data: &Value) -> Reply {
    app.dispatch(&request(path, method, None, None, data), proof::AT)
        .reply
}

fn assert_public_input_guards(app: &mut Hosted, path: &str, data: &Value) {
    let mut unsigned = request(path, "POST", None, None, data);
    unsigned.headers.retain(|(k, _)| k != "Origin");
    assert_eq!(app.dispatch(&unsigned, proof::AT).reply.status, 403);
    let mut oversize = request(path, "POST", None, None, data);
    oversize.body = vec![b' '; 8193];
    assert_eq!(app.dispatch(&oversize, proof::AT).reply.status, 413);
    for (key, value) in [("owner_id", json!(1)), ("consent", json!(false))] {
        let mut forged = data.clone();
        forged[key] = value;
        assert_eq!(public(app, path, "POST", &forged).status, 400);
    }
}

#[test]
fn public_lead_intake_keeps_access_and_membership_boundaries() {
    let (_temp, mut app) = fixture();
    let admin = Operator::signed_in(&mut app, "alice@example.invalid", "alpha");
    let viewer = Operator::signed_in(&mut app, "carol@example.invalid", "alpha");
    let editor = Operator::signed_in(&mut app, "bob@example.invalid", "beta");
    let enable = json!({"enabled":true});
    for (path, method) in [
        ("/api/leads", "GET"),
        ("/api/lead-form", "GET"),
        ("/api/lead-form", "POST"),
    ] {
        assert_eq!(public(&mut app, path, method, &enable).status, 401);
        assert_eq!(viewer.send(&mut app, path, method, &enable).status, 403);
    }
    assert_eq!(
        editor
            .send(&mut app, "/api/lead-form", "POST", &enable)
            .status,
        403
    );
    let active = admin.send(&mut app, "/api/lead-form", "POST", &enable);
    assert_eq!(active.status, 200);
    let data: Value = serde_json::from_slice(&active.body).unwrap();
    let key = data["public_key"].as_str().unwrap();
    let path = format!("/forms/alpha/{key}");
    for (target, status) in [
        (path.clone(), 200),
        ("/forms/intake.js".into(), 200),
        (format!("/forms/beta/{key}"), 404),
        (format!("{path}/extra"), 401),
    ] {
        assert_eq!(
            public(&mut app, &target, "GET", &Value::Null).status,
            status
        );
    }
    let data = json!({"submission_key":"0123456789abcdef0123456789abcdef","company_name":"合成受付商事","contact_name":"テスト担当","email":"safe@example.invalid","consent":true});
    assert_public_input_guards(&mut app, &path, &data);
    for _ in 0..2 {
        let accepted = public(&mut app, &path, "POST", &data);
        assert_eq!(accepted.status, 202);
        assert_eq!(
            serde_json::from_slice::<Value>(&accepted.body).unwrap(),
            json!({"accepted":true})
        );
    }
    let mut corrected = data.clone();
    corrected["email"] = "another@example.invalid".into();
    assert_eq!(public(&mut app, &path, "POST", &corrected).status, 409);
    let inbox = admin.send(&mut app, "/api/leads", "GET", &Value::Null);
    let records: Value = serde_json::from_slice(&inbox.body).unwrap();
    assert_eq!(records["items"].as_array().unwrap().len(), 1);
    assert!(records["items"][0].get("email").is_none());
    let beta = editor.send(&mut app, "/api/leads", "GET", &Value::Null);
    assert_eq!(
        serde_json::from_slice::<Value>(&beta.body).unwrap()["items"],
        json!([])
    );
    let outside: i64 = app
        .store
        .conn
        .query_row("SELECT id FROM users WHERE username='editor'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let invalid_assignment = json!({"revision":1,"owner_id":outside});
    assert_eq!(
        admin
            .send(&mut app, "/api/leads/1/assign", "POST", &invalid_assignment)
            .status,
        400
    );
    assert_eq!(
        admin
            .send(
                &mut app,
                "/api/lead-form",
                "POST",
                &json!({"enabled":false})
            )
            .status,
        200
    );
    assert_eq!(public(&mut app, &path, "POST", &data).status, 404);
}
