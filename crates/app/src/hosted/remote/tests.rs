use super::{
    config::{EndpointPolicy, Endpoints, RemoteConfig},
    store::RemoteStore,
};

mod sqld {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/sqld.rs"
    ));
}

fn endpoints(control: &sqld::Sqld) -> Endpoints {
    let config = br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{}}"#;
    RemoteConfig::parse(config)
        .unwrap()
        .resolve_with(
            |name| {
                Some(if name.ends_with("URL") {
                    control.endpoint()
                } else {
                    "synthetic-fixture-token".into()
                })
            },
            EndpointPolicy::LoopbackTests,
        )
        .unwrap()
}

#[test]
fn control_initialization_records_remote_ready_identity_without_file_pragmas() {
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    let kind: String = store
        .control
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key='database_kind'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "control");
    let values = store
        .conn
        .prepare("SELECT value FROM meta")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        values
            .iter()
            .all(|value| !value.contains("synthetic-fixture-token")
                && !value.contains(&server.endpoint()))
    );
    for pragma in ["PRAGMA application_id", "PRAGMA user_version"] {
        let value: i64 = store
            .control
            .conn
            .query_row(pragma, [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 0);
    }
}

use crate::hosted::{Login, LoginOutcome, Role};
use meibo_core::{
    RemoteIdentity,
    types::{CompanyId, DataOrigin, Field, Filters},
};
use meibo_db::{Connection, params};
use std::collections::BTreeMap;

const PASSWORD: &str = "synthetic-test-password-only";
const AT: i64 = 1_800_000_000;

struct Fixture {
    control: sqld::Sqld,
    alpha: sqld::Sqld,
    beta: sqld::Sqld,
}
impl Fixture {
    fn start() -> Self {
        Self {
            control: sqld::Sqld::start().unwrap(),
            alpha: sqld::Sqld::start().unwrap(),
            beta: sqld::Sqld::start().unwrap(),
        }
    }
    fn endpoints(&self) -> Endpoints {
        let bytes = br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"},"beta":{"url_env":"MEIBO_DB_BETA_URL","token_env":"MEIBO_DB_BETA_TOKEN"}}}"#;
        let values = BTreeMap::from([
            ("MEIBO_DB_CONTROL_URL", self.control.endpoint()),
            ("MEIBO_DB_ALPHA_URL", self.alpha.endpoint()),
            ("MEIBO_DB_BETA_URL", self.beta.endpoint()),
        ]);
        RemoteConfig::parse(bytes)
            .unwrap()
            .resolve_with(
                |name| {
                    if name.ends_with("TOKEN") {
                        Some("synthetic-fixture-token".into())
                    } else {
                        values.get(name).cloned()
                    }
                },
                EndpointPolicy::LoopbackTests,
            )
            .unwrap()
    }
    fn store(&self) -> RemoteStore {
        let mut store = RemoteStore::initialize(self.endpoints()).unwrap();
        for tenant in ["alpha", "beta"] {
            store
                .add_empty_tenant(tenant, DataOrigin::Synthetic)
                .unwrap();
        }
        store.add_user("operator", PASSWORD).unwrap();
        store
            .set_member("operator", "alpha", Some(Role::Admin))
            .unwrap();
        store
            .set_member("operator", "beta", Some(Role::Viewer))
            .unwrap();
        store
    }
    fn raw(&self, tenant: &str) -> Connection {
        let endpoints = self.endpoints();
        Connection::open_remote(if tenant == "control" {
            endpoints.control
        } else {
            endpoints.tenants[tenant].clone()
        })
        .unwrap()
    }
}

fn login(store: &mut RemoteStore, tenant: &str, name: &str, password: &str, at: i64) -> Login {
    match store.login(tenant, name, password, at).unwrap() {
        LoginOutcome::Success(login) => login,
        _ => panic!("synthetic login should succeed"),
    }
}

fn seed_company(fixture: &Fixture, tenant: &str, name: &str) {
    let mut conn = fixture.raw(tenant);
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO companies(id,name,name_key,prefecture_code,created_at,updated_at) VALUES(1,?1,?1,13,?2,?2)", params![name, AT]).unwrap();
    tx.execute_batch("UPDATE counts SET n=(SELECT count(*) FROM companies WHERE status=counts.status AND has_phone=counts.has_phone); INSERT INTO name_fts(name_fts) VALUES('rebuild');").unwrap();
    tx.commit().unwrap();
}

#[test]
fn restart_preserves_two_tenants_three_roles_identity_and_sessions() {
    let fixture = Fixture::start();
    let mut store = fixture.store();
    for (name, role) in [("viewer", Role::Viewer), ("editor", Role::Editor)] {
        store.add_user(name, PASSWORD).unwrap();
        store.set_member(name, "alpha", Some(role)).unwrap();
        let session = login(&mut store, "alpha", name, PASSWORD, AT);
        assert!(session.identity.role == role);
        assert!(matches!(
            store.login("beta", name, PASSWORD, AT).unwrap(),
            LoginOutcome::Invalid
        ));
    }
    seed_company(&fixture, "alpha", "合成企業Alpha");
    seed_company(&fixture, "beta", "合成企業Beta");
    let alpha = login(&mut store, "alpha", "operator", PASSWORD, AT);
    let beta = login(&mut store, "beta", "operator", PASSWORD, AT);
    assert!(alpha.identity.role == Role::Admin);
    assert!(beta.identity.role == Role::Viewer);
    assert_ne!(alpha.identity.scope, beta.identity.scope);
    let service = (store.database_id().to_owned(), store.instance().to_owned());
    let audit_count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
        .unwrap();
    let digest: Vec<u8> = store
        .conn
        .query_row(
            "SELECT digest FROM sessions WHERE tenant='alpha' AND user_id=?1",
            [alpha.identity.user_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(digest.len(), 32);
    assert_ne!(digest, alpha.token.as_bytes());
    let hashes = store
        .conn
        .prepare("SELECT password_hash FROM users ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        hashes
            .iter()
            .all(|hash| hash.starts_with("$argon2id$") && !hash.contains(PASSWORD))
    );
    assert_ne!(hashes[0], hashes[1]);
    drop(store);
    let store = RemoteStore::open(fixture.endpoints()).unwrap();
    assert_eq!(store.database_id(), service.0);
    assert_eq!(store.instance(), service.1);
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        audit_count
    );
    for session in [&alpha, &beta] {
        let identity = store
            .authenticate(&session.token, AT + 60)
            .unwrap()
            .unwrap();
        assert_eq!(identity.scope, session.identity.scope);
        assert_eq!(identity.tenant, session.identity.tenant);
        let engine = store.open_tenant(&identity.tenant).unwrap();
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 1);
        let expected = if identity.tenant == "alpha" {
            "合成企業Alpha"
        } else {
            "合成企業Beta"
        };
        let detail = engine
            .company_detail(CompanyId(1), &vec![Field::Name])
            .unwrap();
        assert_eq!(detail.company.name.as_deref(), Some(expected));
    }
    assert!(store.open_tenant("unknown").is_err());
    assert!(store.open_tenant("../alpha").is_err());
}

#[test]
fn empty_creation_is_inaccessible_until_validated_atomic_registration() {
    let fixture = Fixture::start();
    let mut store = RemoteStore::initialize(fixture.endpoints()).unwrap();
    store.add_user("operator", PASSWORD).unwrap();
    let (identity, engine) = store.create_tenant("alpha", DataOrigin::Synthetic).unwrap();
    assert_eq!(engine.count(&Filters::default()).unwrap().n, 0);
    drop(engine);
    assert!(store.open_tenant("alpha").is_err());
    assert!(
        store
            .set_member("operator", "alpha", Some(Role::Admin))
            .is_err()
    );
    let wrong = RemoteIdentity::new("wrong-identity", "crm").unwrap();
    assert!(store.register_tenant("alpha", &wrong).is_err());
    assert!(!store.tenant_exists("alpha").unwrap());
    store.register_tenant("alpha", &identity).unwrap();
    assert_eq!(store.registered_tenants().unwrap().len(), 1);
    assert!(store.create_tenant("alpha", DataOrigin::Synthetic).is_err());
    assert!(
        store
            .add_empty_tenant("unconfigured", DataOrigin::Synthetic)
            .is_err()
    );
    assert!(
        store
            .add_empty_tenant("../escape", DataOrigin::Synthetic)
            .is_err()
    );
    drop(store);
    let store = RemoteStore::open(fixture.endpoints()).unwrap();
    assert_eq!(
        store
            .open_tenant("alpha")
            .unwrap()
            .count(&Filters::default())
            .unwrap()
            .n,
        0
    );
    assert!(store.open_tenant("beta").is_err());
    let objects: i64 = fixture
        .raw("beta")
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(objects, 0);
}

#[test]
fn sessions_expire_logout_and_revoke_with_shared_authentication_rules() {
    let fixture = Fixture::start();
    let mut store = fixture.store();
    let session = login(&mut store, "alpha", "operator", PASSWORD, AT);
    assert!(
        store
            .authenticate(&session.token, AT - 1)
            .unwrap()
            .is_none()
    );
    let session = login(&mut store, "alpha", "operator", PASSWORD, AT);
    assert!(
        store
            .authenticate(&session.token, AT + 60)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .authenticate(&session.token, AT + 1860)
            .unwrap()
            .is_none()
    );
    let session = login(&mut store, "alpha", "operator", PASSWORD, AT);
    for minute in 1..16 {
        assert!(
            store
                .authenticate(&session.token, AT + minute * 1799)
                .unwrap()
                .is_some()
        );
    }
    assert!(
        store
            .authenticate(&session.token, AT + 8 * 3600)
            .unwrap()
            .is_none()
    );
    let session = login(&mut store, "alpha", "operator", PASSWORD, AT);
    store.logout(&session.token).unwrap();
    assert!(
        store
            .authenticate(&session.token, AT + 1)
            .unwrap()
            .is_none()
    );
    let alpha = login(&mut store, "alpha", "operator", PASSWORD, AT);
    let beta = login(&mut store, "beta", "operator", PASSWORD, AT);
    store.revoke_user("operator").unwrap();
    assert!(store.authenticate(&alpha.token, AT + 1).unwrap().is_none());
    assert!(store.authenticate(&beta.token, AT + 1).unwrap().is_none());
}

#[test]
fn membership_password_disable_and_throttle_changes_are_persistent() {
    let fixture = Fixture::start();
    let mut store = fixture.store();
    let old = login(&mut store, "alpha", "operator", PASSWORD, AT);
    store
        .set_member("operator", "alpha", Some(Role::Editor))
        .unwrap();
    assert!(store.authenticate(&old.token, AT + 1).unwrap().is_none());
    let editor = login(&mut store, "alpha", "operator", PASSWORD, AT);
    assert!(editor.identity.role == Role::Editor);
    store.set_member("operator", "alpha", None).unwrap();
    assert!(store.authenticate(&editor.token, AT + 1).unwrap().is_none());
    assert!(matches!(
        store.login("alpha", "operator", PASSWORD, AT).unwrap(),
        LoginOutcome::Invalid
    ));
    let beta = login(&mut store, "beta", "operator", PASSWORD, AT);
    store
        .set_password("operator", "a-different-fixture-password")
        .unwrap();
    assert!(store.authenticate(&beta.token, AT + 1).unwrap().is_none());
    assert!(matches!(
        store.login("beta", "operator", PASSWORD, AT).unwrap(),
        LoginOutcome::Invalid
    ));
    let beta = login(
        &mut store,
        "beta",
        "operator",
        "a-different-fixture-password",
        AT,
    );
    store.disable_user("operator").unwrap();
    assert!(store.authenticate(&beta.token, AT + 1).unwrap().is_none());
    drop(store);
    let mut store = RemoteStore::open(fixture.endpoints()).unwrap();
    assert!(matches!(
        store
            .login("beta", "operator", "a-different-fixture-password", AT)
            .unwrap(),
        LoginOutcome::Invalid
    ));
    store.add_user("rate-limited", PASSWORD).unwrap();
    store
        .set_member("rate-limited", "alpha", Some(Role::Viewer))
        .unwrap();
    for _ in 0..5 {
        assert!(matches!(
            store
                .login("alpha", "rate-limited", "incorrect-password", AT)
                .unwrap(),
            LoginOutcome::Invalid
        ));
    }
    drop(store);
    let mut store = RemoteStore::open(fixture.endpoints()).unwrap();
    assert!(matches!(
        store
            .login("alpha", "rate-limited", PASSWORD, AT + 1)
            .unwrap(),
        LoginOutcome::Throttled
    ));
    assert!(matches!(
        store
            .login("alpha", "rate-limited", PASSWORD, AT + 901)
            .unwrap(),
        LoginOutcome::Success(_)
    ));
    let operations = store.audit_rows(None, 200).unwrap();
    for operation in [
        "membership.change",
        "password.change",
        "user.disable",
        "login",
    ] {
        assert!(operations.iter().any(|row| row["operation"] == operation));
    }
}

#[test]
fn duplicate_database_ids_reject_registration_and_roll_back_tenant_row() {
    let fixture = Fixture::start();
    let mut store = RemoteStore::initialize(fixture.endpoints()).unwrap();
    store
        .add_empty_tenant("alpha", DataOrigin::Synthetic)
        .unwrap();
    let id = store.registered_tenants().unwrap().remove(0).1;
    let (_, engine) = store.create_tenant("beta", DataOrigin::Synthetic).unwrap();
    drop(engine);
    fixture
        .raw("beta")
        .execute(
            "UPDATE meta SET value=?1 WHERE key='database_id'",
            [id.database_id()],
        )
        .unwrap();
    assert!(store.register_tenant("beta", &id).is_err());
    assert!(!store.tenant_exists("beta").unwrap());
    let mapped: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM remote_tenants WHERE slug='beta'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mapped, 0);
    assert_eq!(store.registered_tenants().unwrap().len(), 1);
}

#[test]
fn missing_config_swapped_endpoints_and_wrong_mapping_refuse_open_and_membership() {
    let fixture = Fixture::start();
    let mut store = fixture.store();
    let mut missing = fixture.endpoints();
    missing.tenants.remove("beta");
    assert!(RemoteStore::open(missing).is_err());
    let mut swapped = fixture.endpoints();
    let alpha = swapped.tenants["alpha"].clone();
    swapped
        .tenants
        .insert("alpha".into(), swapped.tenants["beta"].clone());
    swapped.tenants.insert("beta".into(), alpha);
    assert!(RemoteStore::open(swapped).is_err());
    store
        .conn
        .execute(
            "UPDATE remote_tenants SET database_id='wrong-identity' WHERE slug='alpha'",
            [],
        )
        .unwrap();
    assert!(RemoteStore::open(fixture.endpoints()).is_err());
    assert!(store.open_tenant("alpha").is_err());
    assert!(
        store
            .set_member("operator", "alpha", Some(Role::Viewer))
            .is_err()
    );
    let role: String = store
        .conn
        .query_row(
            "SELECT role FROM memberships WHERE tenant='alpha'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(role, "admin");
}

#[test]
fn control_markers_and_service_identity_are_strict_and_extra_meta_is_opaque() {
    let fixture = Fixture::start();
    let store = RemoteStore::initialize(fixture.endpoints()).unwrap();
    store.conn.execute("INSERT INTO meta(key,value) VALUES('opaque','https://synthetic.invalid/private-value; DROP TABLE users')", []).unwrap();
    assert!(RemoteStore::open(fixture.endpoints()).is_ok());
    for (key, invalid) in [
        ("remote_format", "future-secret-format"),
        ("schema_version", "2"),
        ("database_kind", "crm"),
        ("state", "restoring"),
        ("database_id", "bad-secret-id"),
        ("instance", "bad-secret-instance"),
        ("dummy_hash", "private-secret-hash"),
    ] {
        let original: String = store
            .conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![key, invalid],
            )
            .unwrap();
        let error = RemoteStore::open(fixture.endpoints()).err().unwrap();
        assert_eq!(
            error.to_string(),
            "認証DBの保存形式・組織対応を確認してください。"
        );
        assert!(!format!("{error:?}").contains(invalid));
        store
            .conn
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![key, original],
            )
            .unwrap();
    }
    store
        .conn
        .execute(
            "UPDATE meta SET value='11111111111111111111111111111111' WHERE key='instance'",
            [],
        )
        .unwrap();
    assert!(store.create_tenant("alpha", DataOrigin::Synthetic).is_err());
    store
        .conn
        .execute("DELETE FROM meta WHERE key='remote_format'", [])
        .unwrap();
    assert!(RemoteStore::open(fixture.endpoints()).is_err());
}

#[test]
fn malformed_crm_markers_and_absent_registration_never_fall_back_to_local() {
    let fixture = Fixture::start();
    let store = fixture.store();
    let raw = fixture.raw("alpha");
    for (key, invalid) in [
        ("remote_format", "wrong"),
        ("schema_version", "999"),
        ("database_kind", "control"),
        ("data_origin", "unknown"),
    ] {
        let original: String = raw
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .unwrap();
        raw.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, invalid],
        )
        .unwrap();
        assert!(RemoteStore::open(fixture.endpoints()).is_err());
        assert!(store.open_tenant("alpha").is_err());
        raw.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, original],
        )
        .unwrap();
    }
    store
        .conn
        .execute("DELETE FROM remote_tenants WHERE slug='alpha'", [])
        .unwrap();
    assert!(RemoteStore::open(fixture.endpoints()).is_err());
    assert!(store.open_tenant("alpha").is_err());
}

#[test]
fn initialization_refuses_nonempty_or_wrong_kind_database_without_changes() {
    let server = sqld::Sqld::start().unwrap();
    let settings = endpoints(&server);
    let raw = Connection::open_remote(settings.control.clone()).unwrap();
    raw.execute_batch(
        "CREATE TABLE existing(value TEXT); INSERT INTO existing VALUES('synthetic-preserved');",
    )
    .unwrap();
    assert!(RemoteStore::initialize(settings.clone()).is_err());
    let error = RemoteStore::open(settings).err().unwrap();
    assert!(
        error
            .source()
            .is_some_and(|cause| cause.downcast_ref::<meibo_db::Error>().is_some())
    );
    let value: String = raw
        .query_row("SELECT value FROM existing", [], |r| r.get(0))
        .unwrap();
    assert_eq!(value, "synthetic-preserved");
    let count: i64 = raw
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let fixture = Fixture::start();
    let store = fixture.store();
    assert!(RemoteStore::initialize(fixture.endpoints()).is_err());
    let mut crm_control = fixture.endpoints();
    crm_control.control = crm_control.tenants["alpha"].clone();
    assert!(RemoteStore::open(crm_control).is_err());
    assert_eq!(store.registered_tenants().unwrap().len(), 2);
}

#[test]
fn connection_failure_diagnostics_preserve_sources_without_credentials() {
    let server = sqld::Sqld::start().unwrap();
    let settings = endpoints(&server);
    let endpoint = server.endpoint();
    drop(server);
    let error = RemoteStore::open(settings).err().unwrap();
    assert!(error.downcast_ref::<meibo_db::Error>().is_some());
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(&endpoint));
    assert!(!rendered.contains("synthetic-fixture-token"));
}
