use super::*;

// REQ 20.2
// REQ 20.8
#[test]
fn due_notifications_use_the_signed_in_member_and_active_organization() {
    let (_dir, mut store) = fixture();
    store.add_user("colleague", PASSWORD).unwrap();
    store
        .set_member("colleague", "alpha", Some(Role::Editor))
        .unwrap();
    let alice = login(&mut store, "alpha");
    let beta = login(&mut store, "beta");
    let LoginOutcome::Success(bob) = store.login("alpha", "colleague", PASSWORD, AT).unwrap()
    else {
        panic!("fixture login");
    };
    let mut app = Hosted::new(store, config());
    let payload = json!({"follow_up":"absence","request_key":"synthetic-user-callback","note":"合成","next_action_at":200});
    let saved = app.dispatch(
        &request(
            "/api/companies/1/activities",
            "POST",
            Some(&alice),
            &payload,
        ),
        AT,
    );
    assert_eq!(saved.reply.status, 200);
    for (who, count) in [(&alice, 1), (&bob, 0), (&beta, 0)] {
        let mut input = request("/api/followups", "GET", Some(who), &json!(null));
        input
            .headers
            .push(("X-Meibo-User".into(), alice.identity.user_id.to_string()));
        let response = app.dispatch(&input, AT);
        assert_eq!(response.reply.status, 200);
        let page: serde_json::Value = serde_json::from_slice(&response.reply.body).unwrap();
        assert_eq!(page["total"], count);
    }
}
