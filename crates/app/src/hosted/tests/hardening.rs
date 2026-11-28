use super::*;
use serde_json::Value;
use std::fs;

fn use_fast_dummy_hash(store: &Store) {
    use argon2::{
        Algorithm, Argon2, Params, Version,
        password_hash::{PasswordHasher, SaltString},
    };
    // Unknown synthetic users still exercise password verification.
    let fast = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(8, 1, 1, None).unwrap(),
    );
    let salt = SaltString::encode_b64(b"synthetic-test-salt").unwrap();
    let dummy = fast
        .hash_password(b"synthetic dummy", &salt)
        .unwrap()
        .to_string();
    store
        .conn
        .execute("UPDATE meta SET value=?1 WHERE key='dummy_hash'", [dummy])
        .unwrap();
}

#[test]
fn local_login_burst_recovers_for_another_tenant() {
    let (_temp, store) = fixture();
    use_fast_dummy_hash(&store);
    let mut app = Hosted::new(store, config());
    let attempt = |tenant: &str, name: &str, password: &str| {
        request(
            "/auth/login",
            "POST",
            None,
            &json!({
                "tenant": tenant, "username": name, "password": password
            }),
        )
    };
    let valid = attempt("beta", "operator", PASSWORD);
    let denied = app
        .dispatch(&attempt("alpha", "operator", "incorrect"), AT)
        .reply;
    assert_eq!(denied.status, 401);
    for n in 0..99 {
        let reply = app
            .dispatch(&attempt("alpha", &format!("absent-{n}"), "incorrect"), AT)
            .reply;
        assert_eq!((reply.status, &reply.body), (401, &denied.body));
    }
    assert_eq!(app.dispatch(&valid, AT).reply.status, 429);
    let login_page = request("/login", "GET", None, &Value::Null);
    assert_eq!(app.dispatch(&login_page, AT).reply.status, 200);
    for n in 99..103 {
        let reply = app.dispatch(
            &attempt("alpha", &format!("absent-{n}"), "incorrect"),
            AT + 1,
        );
        assert_eq!(reply.reply.status, 401);
    }
    assert_eq!(app.dispatch(&valid, AT + 1).reply.status, 200);
    assert_eq!(app.dispatch(&valid, AT + 1).reply.status, 429);
}

#[test]
fn failed_login_audit_survives_eviction_and_resumes_after_fifteen_minutes() {
    let (_temp, mut store) = fixture();
    use_fast_dummy_hash(&store);
    let audit_counts = |store: &Store| -> (i64, i64, i64) {
        store
            .conn
            .query_row(
                "SELECT count(*), count(*) FILTER (WHERE operation='login'),
                        count(*) FILTER (WHERE operation='login.audit.suppressed')
                 FROM audit WHERE outcome='denied'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    };
    assert!(matches!(
        store.login("alpha", "operator", "incorrect", AT).unwrap(),
        LoginOutcome::Invalid
    ));
    login(&mut store, "beta");
    // One lockout expires halfway through the audit window. The remaining 4095 accounts
    // have one failure each and are newer than the audit bucket, so it would be evicted first.
    store
        .conn
        .execute(
            "INSERT INTO login_attempts VALUES(?1,?2,5)",
            (auth::hex(&auth::digest(b"operator")), AT - 450),
        )
        .unwrap();
    store
        .conn
        .execute(
            "WITH RECURSIVE names(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM names WHERE n<4095)
             INSERT INTO login_attempts SELECT printf('%064x',n), ?1, 1 FROM names",
            [AT + 1],
        )
        .unwrap();
    assert!(matches!(
        store.login("beta", "operator", PASSWORD, AT + 1).unwrap(),
        LoginOutcome::Throttled
    ));
    for failure in 2..=300 {
        let at = AT + (failure - 1) * 3;
        assert!(matches!(
            store
                .login("alpha", &format!("absent-{failure}"), "incorrect", at)
                .unwrap(),
            LoginOutcome::Invalid
        ));
        match failure {
            100 => assert_eq!(audit_counts(&store), (100, 100, 0)),
            101 => assert_eq!(audit_counts(&store), (101, 100, 1)),
            _ => {}
        }
    }
    assert_eq!(audit_counts(&store), (101, 100, 1));
    assert!(matches!(
        store.login("beta", "operator", PASSWORD, AT + 899).unwrap(),
        LoginOutcome::Success(_)
    ));
    assert!(matches!(
        store
            .login("alpha", "absent-next", "incorrect", AT + 900)
            .unwrap(),
        LoginOutcome::Invalid
    ));
    assert_eq!(audit_counts(&store), (102, 101, 1));
}

#[test]
fn full_failure_table_evicts_the_oldest_single_failure_and_preserves_lockouts() {
    let (_temp, mut store) = fixture();
    // Existing roots can still contain the retired service-wide counter.
    store
        .conn
        .execute("INSERT INTO login_attempts VALUES('global',?1,100)", [AT])
        .unwrap();
    for _ in 0..5 {
        assert!(matches!(
            store.login("alpha", "operator", "incorrect", AT).unwrap(),
            LoginOutcome::Invalid
        ));
    }
    store
        .conn
        .execute(
            "WITH RECURSIVE names(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM names WHERE n<4095)
             INSERT INTO login_attempts SELECT printf('%064x',n), ?1 + (n <> 4095), 1 FROM names",
            [AT + 1],
        )
        .unwrap();
    for _ in 0..5 {
        assert!(matches!(
            store
                .login("alpha", "new-account", "incorrect", AT + 3)
                .unwrap(),
            LoginOutcome::Invalid
        ));
    }
    let (count, oldest, next): (i64, bool, bool) = store
        .conn
        .query_row(
            "SELECT count(*), count(*) FILTER (WHERE bucket=printf('%064x',4095)),
                              count(*) FILTER (WHERE bucket=printf('%064x',1)) FROM login_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((count, oldest, next), (4096 + 1, false, true)); // Accounts plus the audit bucket.
    for name in ["operator", "new-account"] {
        assert!(matches!(
            store.login("alpha", name, PASSWORD, AT + 4).unwrap(),
            LoginOutcome::Throttled
        ));
    }
    store.add_user("reader", PASSWORD).unwrap();
    store
        .set_member("reader", "beta", Some(Role::Viewer))
        .unwrap();
    assert!(matches!(
        store.login("beta", "reader", PASSWORD, AT + 4).unwrap(),
        LoginOutcome::Success(_)
    ));
}

#[test]
fn saved_fingerprint_rejects_exchanged_tenant_databases_with_matching_file_hashes() {
    let (temp, store) = fixture();
    let snapshot = temp.path().join("snapshot");
    let trusted = backup::create(&store, &snapshot).unwrap();
    let path = snapshot.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let alpha = fs::read(snapshot.join("tenants/alpha.sqlite3")).unwrap();
    let beta = fs::read(snapshot.join("tenants/beta.sqlite3")).unwrap();
    for (name, bytes) in [
        ("tenants/alpha.sqlite3", beta),
        ("tenants/beta.sqlite3", alpha),
    ] {
        let file = manifest["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|f| f["name"] == name)
            .unwrap();
        file["sha256"] = json!(auth::hex(&auth::digest(&bytes)));
        fs::write(snapshot.join(name), bytes).unwrap();
    }
    fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let root = temp.path().join("restored");
    assert_eq!(
        backup::restore(&snapshot, &root, &trusted)
            .unwrap_err()
            .to_string(),
        "backup目録の指紋が一致しません。"
    );
    assert!(!root.exists());
}

#[test]
fn invalid_tenant_database_leaves_only_the_preexisting_directory() {
    let (temp, store) = fixture();
    let snapshot = temp.path().join("snapshot");
    backup::create(&store, &snapshot).unwrap();
    let path = snapshot.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let name = "tenants/beta.sqlite3";
    let invalid = b"invalid synthetic database";
    fs::write(snapshot.join(name), invalid).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["name"] == name)
        .unwrap();
    entry["sha256"] = json!(auth::hex(&auth::digest(invalid)));
    let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(&path, &bytes).unwrap();
    let trusted = auth::hex(&auth::digest(&bytes));
    let parent = temp.path().join("output");
    fs::create_dir(&parent).unwrap();
    let other = parent.join(".meibo-restore-other");
    fs::create_dir(&other).unwrap();
    let root = parent.join("restored");
    assert!(backup::restore(&snapshot, &root, &trusted).is_err());
    assert_eq!(
        fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>(),
        [other]
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(fs::read(snapshot.join(name)).unwrap(), invalid);
}
