use std::sync::Mutex;

use meibo_core::types::DataOrigin;
use meibo_db::{Connection, RemoteOptions};
use serde_json::{Value, json};

use super::{
    config::{EndpointPolicy, Endpoints, RemoteConfig},
    store::RemoteStore,
};
use crate::{
    hosted::{Hosted, HostedConfig, HostedReply, Login, LoginOutcome, Role},
    server::Input,
};

#[allow(dead_code)]
mod client {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/client.rs"
    ));
}
#[allow(dead_code)]
mod proxy {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/proxy.rs"
    ));
}
#[allow(dead_code)]
mod sqld {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/sqld.rs"
    ));
}

const PASSWORD: &str = "synthetic-runtime-password-only";
const TOKEN: &str = "synthetic-runtime-token-only";
const AT: i64 = 1_800_000_000;
static FIXTURES: Mutex<()> = Mutex::new(());

fn endpoints(control: &str, tenant: &str) -> Endpoints {
    RemoteConfig::parse(br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"}}}"#).unwrap()
        .resolve_with(|key| Some(if key.ends_with("TOKEN") { TOKEN.into() } else if key == "MEIBO_DB_CONTROL_URL" { control.into() } else { tenant.into() }), EndpointPolicy::LoopbackTests).unwrap()
}

struct Fixture {
    control: sqld::Sqld,
    tenant: sqld::Sqld,
    login: Login,
}
impl Fixture {
    fn new() -> Self {
        let control = sqld::Sqld::start().unwrap();
        let tenant = sqld::Sqld::start().unwrap();
        let mut store =
            RemoteStore::initialize(endpoints(&control.endpoint(), &tenant.endpoint())).unwrap();
        store
            .add_empty_tenant("alpha", DataOrigin::Synthetic)
            .unwrap();
        store.add_user("admin", PASSWORD).unwrap();
        store
            .set_member("admin", "alpha", Some(Role::Admin))
            .unwrap();
        let LoginOutcome::Success(login) = store.login("alpha", "admin", PASSWORD, AT).unwrap()
        else {
            panic!("synthetic login");
        };
        Self {
            control,
            tenant,
            login,
        }
    }
    fn store(&self) -> RemoteStore {
        RemoteStore::open(endpoints(&self.control.endpoint(), &self.tenant.endpoint())).unwrap()
    }
    fn service(control: &str, tenant: &str) -> Hosted<RemoteStore> {
        Hosted::new(
            RemoteStore::open(endpoints(control, tenant)).unwrap(),
            HostedConfig::parse("https://crm.example.test", "127.0.0.1:0", false).unwrap(),
        )
    }
    fn request(&self, method: &str, target: &str, value: &Value) -> Input {
        Input {
            method: method.into(),
            target: target.into(),
            body: if method == "POST" {
                value.to_string().into_bytes()
            } else {
                vec![]
            },
            headers: vec![
                ("Host".into(), "crm.example.test".into()),
                ("Origin".into(), "https://crm.example.test".into()),
                ("Content-Type".into(), "application/json".into()),
                ("X-Meibo-Local".into(), "1".into()),
                (
                    "Cookie".into(),
                    format!("__Host-meibo-session={}", self.login.token),
                ),
                ("X-Meibo-Context".into(), self.login.identity.scope.clone()),
            ],
        }
    }
    fn raw_tenant(&self) -> Connection {
        Connection::open_remote(
            RemoteOptions::new(self.tenant.endpoint(), TOKEN).allow_loopback_http_for_tests(),
        )
        .unwrap()
    }
}

fn unknown(reply: &HostedReply) {
    assert_eq!(reply.reply.status, 503);
    let value: Value = serde_json::from_slice(&reply.reply.body).unwrap();
    assert_eq!(value["code"], "outcome_unknown");
    assert_eq!(value["error"], super::operation::UNKNOWN_MESSAGE);
}

#[test]
fn converted_crm_failure_keeps_gate_and_stops_before_audit_or_reuse() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let proxy = proxy::FaultProxy::start(fixture.tenant.address()).unwrap();
    let mut web = Fixture::service(&fixture.control.endpoint(), &proxy.endpoint());
    assert_eq!(
        web.dispatch(&fixture.request("GET", "/api/summary", &Value::Null), AT)
            .reply
            .status,
        200
    );
    proxy.arm(proxy::Fault::LoseCommitResponse);
    let reply = web.dispatch(
        &fixture.request(
            "POST",
            "/api/companies",
            &json!({"name":"合成の検証企業","prefecture":13}),
        ),
        AT,
    );
    unknown(&reply);
    assert_eq!(proxy.triggered(), 1);
    let sent = proxy.requests();
    let direct = fixture.store();
    assert!(direct.gate_status().unwrap().is_some());
    let audits: i64 = direct
        .conn
        .query_row(
            "SELECT count(*) FROM audit WHERE operation='crm.write'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        audits, 0,
        "uncertain CRM completion must not be recorded as a known failure"
    );
    unknown(&web.dispatch(&fixture.request("GET", "/api/summary", &Value::Null), AT));
    assert_eq!(proxy.requests(), sent);
    drop(web);
    assert_eq!(proxy.requests(), sent);
    assert_eq!(
        fixture
            .raw_tenant()
            .query_row("SELECT count(*) FROM companies", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let observed = direct.gate_status().unwrap().unwrap();
    direct.recover_gate(&observed, true).unwrap();
    let mut restarted = Fixture::service(&fixture.control.endpoint(), &proxy.endpoint());
    assert_eq!(
        restarted
            .dispatch(&fixture.request("GET", "/api/summary", &Value::Null), AT)
            .reply
            .status,
        200
    );
}

#[test]
fn failed_engine_constructor_does_not_treat_an_absent_engine_as_healthy() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let proxy = proxy::FaultProxy::start(fixture.tenant.address()).unwrap();
    let mut web = Fixture::service(&fixture.control.endpoint(), &proxy.endpoint());
    proxy.arm(proxy::Fault::LoseCommitResponse);
    unknown(&web.dispatch(&fixture.request("GET", "/api/summary", &Value::Null), AT));
    assert_eq!(proxy.triggered(), 1);
    let sent = proxy.requests();
    assert!(fixture.store().gate_status().unwrap().is_some());
    unknown(&web.dispatch(&fixture.request("GET", "/api/summary", &Value::Null), AT));
    assert_eq!(proxy.requests(), sent);
}

#[test]
fn acquisition_and_control_audit_response_loss_are_uncertain() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for prefix in ["INSERT INTO remote_gate", "INSERT INTO audit"] {
        let fixture = Fixture::new();
        let proxy = proxy::FaultProxy::start(fixture.control.address()).unwrap();
        let mut web = Fixture::service(&proxy.endpoint(), &fixture.tenant.endpoint());
        proxy.arm(proxy::Fault::LoseMatchingWriteResponse(prefix));
        let input = if prefix == "INSERT INTO audit" {
            fixture.request(
                "POST",
                "/api/companies",
                &json!({"name":"合成の確認企業","prefecture":13}),
            )
        } else {
            fixture.request("GET", "/health", &Value::Null)
        };
        unknown(&web.dispatch(&input, AT));
        assert_eq!(proxy.triggered(), 1);
        assert!(fixture.store().gate_status().unwrap().is_some());
        let sent = proxy.requests();
        unknown(&web.dispatch(&fixture.request("GET", "/health", &Value::Null), AT));
        assert_eq!(proxy.requests(), sent);
    }
}

#[test]
fn release_response_loss_does_not_claim_success_or_resend_delete() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let proxy = proxy::FaultProxy::start(fixture.control.address()).unwrap();
    let mut web = Fixture::service(&proxy.endpoint(), &fixture.tenant.endpoint());
    proxy.arm(proxy::Fault::LoseMatchingWriteResponse(
        "DELETE FROM remote_gate",
    ));
    unknown(&web.dispatch(&fixture.request("GET", "/health", &Value::Null), AT));
    assert_eq!(proxy.triggered(), 1);
    let sent = proxy.requests();
    // Protected work was already complete; a lost release acknowledgement can
    // mean the conditional DELETE committed. An operator checks a fresh store.
    assert!(fixture.store().gate_status().unwrap().is_none());
    unknown(&web.dispatch(&fixture.request("GET", "/health", &Value::Null), AT));
    assert_eq!(proxy.requests(), sent);
}

#[test]
fn a_known_crm_validation_failure_releases_the_gate() {
    let _serial = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let mut web = Fixture::service(&fixture.control.endpoint(), &fixture.tenant.endpoint());
    assert_eq!(
        web.dispatch(
            &fixture.request(
                "POST",
                "/api/companies",
                &json!({"name":"","prefecture":13})
            ),
            AT
        )
        .reply
        .status,
        400
    );
    assert!(fixture.store().gate_status().unwrap().is_none());
    assert_eq!(
        web.dispatch(&fixture.request("GET", "/health", &Value::Null), AT)
            .reply
            .status,
        200
    );
}
