use super::*;
use crate::hosted::remote::config::{EndpointPolicy, Endpoints, RemoteConfig};
use meibo_db::params;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
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

mod sqld {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../db/tests/support/sqld.rs"
    ));
}

static FIXTURES: Mutex<()> = Mutex::new(());
fn serial() -> MutexGuard<'static, ()> {
    FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn endpoints(server: &sqld::Sqld) -> Endpoints {
    endpoints_url(&server.endpoint())
}
fn endpoints_url(url: &str) -> Endpoints {
    RemoteConfig::parse(br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{}}"#)
        .unwrap()
        .resolve_with(|key| Some(if key.ends_with("URL") { url.into() } else { "synthetic-gate-token".into() }), EndpointPolicy::LoopbackTests)
        .unwrap()
}

#[test]
fn slot_is_exclusive_and_a_known_owner_can_release() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    assert!(store.gate_status().unwrap().is_none());
    let owner = store.acquire_gate("http.request").unwrap();
    let status = store.gate_status().unwrap().unwrap();
    assert_eq!(&status, owner.status());
    assert_eq!(status.owner_hex().len(), 64);
    let contender = RemoteStore::open(endpoints(&server)).unwrap();
    let error = contender.acquire_gate("cli.user-add").unwrap_err();
    assert_eq!(error.downcast_ref::<GateError>(), Some(&GateError::Busy));
    owner.release(&store).unwrap();
    assert!(store.gate_status().unwrap().is_none());
    contender
        .acquire_gate("cli.user-add")
        .unwrap()
        .release(&contender)
        .unwrap();
}

#[test]
fn explicit_recovery_deletes_only_the_observed_row_and_audits() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    store
        .conn
        .execute(
            "INSERT INTO remote_gate(slot,owner,at,operation) VALUES(1,?1,0,'cli.user-add')",
            params![vec![7_u8; 32]],
        )
        .unwrap();
    let status = store.gate_status().unwrap().unwrap();
    store.recover_gate(&status, true).unwrap();
    assert!(store.gate_status().unwrap().is_none());
    let rows = store.audit_rows(None, 10).unwrap();
    assert!(rows.iter().any(|row| {
        row["operation"] == "gate.recover"
            && row["outcome"]
                .as_str()
                .is_some_and(|value| value.starts_with("success:"))
    }));
}

#[test]
fn an_invalid_gate_row_is_never_reported_as_free() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    store.conn.execute_batch("DROP TABLE remote_gate; CREATE TABLE remote_gate(slot INTEGER,owner BLOB,at INTEGER,operation TEXT); INSERT INTO remote_gate VALUES(2,X'00',0,'invalid');").unwrap();
    let error = store.gate_status().unwrap_err();
    assert_eq!(
        error.downcast_ref::<GateError>(),
        Some(&GateError::InvalidState)
    );
    assert_gate(
        store.acquire_gate("http.request").unwrap_err(),
        GateError::Busy,
    );
}

fn assert_gate(error: Box<dyn Error>, kind: GateError) {
    assert_eq!(
        error.downcast::<GateError>().map(|value| *value).ok(),
        Some(kind)
    );
}

fn remote_kind(error: &(dyn Error + 'static)) -> Option<meibo_db::RemoteErrorKind> {
    if let Some(error) = error.downcast_ref::<meibo_db::Error>() {
        return meibo_db::remote_error_kind(error);
    }
    error.source().and_then(remote_kind)
}

#[test]
fn drop_panic_restart_and_old_timestamps_never_release_or_take_over() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _owner = store.acquire_gate("http.request").unwrap();
        panic!("synthetic interrupted protected operation");
    }));
    assert!(panic.is_err());
    store
        .conn
        .execute("UPDATE remote_gate SET at=0", [])
        .unwrap();
    let observed = store.gate_status().unwrap().unwrap();
    drop(store);
    let reopened = RemoteStore::open(endpoints(&server)).unwrap();
    assert_eq!(reopened.gate_status().unwrap().as_ref(), Some(&observed));
    assert_gate(
        reopened.acquire_gate("cli.user-add").unwrap_err(),
        GateError::Busy,
    );
    assert_gate(
        reopened.recover_gate(&observed, false).unwrap_err(),
        GateError::PriorWorkNotTerminated,
    );
    let mut wrong = observed.clone();
    wrong.owner[0] ^= 1;
    assert_gate(
        reopened.recover_gate(&wrong, true).unwrap_err(),
        GateError::OwnerMismatch,
    );
    wrong = observed.clone();
    wrong.operation = "cli.user-add".into();
    assert_gate(
        reopened.recover_gate(&wrong, true).unwrap_err(),
        GateError::OwnerMismatch,
    );
    assert_eq!(reopened.gate_status().unwrap().as_ref(), Some(&observed));
    reopened.recover_gate(&observed, true).unwrap();
    let guard = reopened.acquire_gate("cli.user-add").unwrap();
    assert_ne!(guard.status().owner, observed.owner);
    guard.release(&reopened).unwrap();
}

#[test]
fn release_and_recovery_bind_the_control_identity_and_exact_owner() {
    let _serial = serial();
    let first = sqld::Sqld::start().unwrap();
    let second = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&first)).unwrap();
    let other = RemoteStore::initialize(endpoints(&second)).unwrap();
    let guard = store.acquire_gate("http.request").unwrap();
    let observed = guard.status().clone();
    assert_gate(guard.release(&other).unwrap_err(), GateError::OwnerMismatch);
    assert_gate(
        other.recover_gate(&observed, true).unwrap_err(),
        GateError::OwnerMismatch,
    );
    assert!(other.gate_status().unwrap().is_none());
    let mut wrong = observed.clone();
    wrong.owner[0] ^= 1;
    assert_gate(
        GateGuard { status: wrong }.release(&store).unwrap_err(),
        GateError::OwnerMismatch,
    );
    let mut wrong = observed.clone();
    wrong.operation = "cli.audit".into();
    assert_gate(
        GateGuard { status: wrong }.release(&store).unwrap_err(),
        GateError::OwnerMismatch,
    );
    assert_eq!(store.gate_status().unwrap().as_ref(), Some(&observed));
    store.recover_gate(&observed, true).unwrap();
}

#[test]
fn recovery_audit_failure_rolls_back_deletion() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    drop(store.acquire_gate("cli.backup").unwrap());
    let observed = store.gate_status().unwrap().unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_gate_audit BEFORE INSERT ON audit WHEN NEW.operation='gate.recover' BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
    assert!(store.recover_gate(&observed, true).is_err());
    assert_eq!(store.gate_status().unwrap().as_ref(), Some(&observed));
    assert!(
        !store
            .audit_rows(None, 20)
            .unwrap()
            .iter()
            .any(|row| row["operation"] == "gate.recover")
    );
    store
        .conn
        .execute_batch("DROP TRIGGER fail_gate_audit")
        .unwrap();
    store.recover_gate(&observed, true).unwrap();
    let rows = store.audit_rows(None, 20).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row["operation"] == "gate.recover")
            .count(),
        1
    );
}

#[test]
fn lost_acquisition_response_persists_owner_and_forbids_resend_or_reuse() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let direct = RemoteStore::initialize(endpoints(&server)).unwrap();
    let proxy = proxy::FaultProxy::start(server.address()).unwrap();
    let store = RemoteStore::open(endpoints_url(&proxy.endpoint())).unwrap();
    proxy.arm(proxy::Fault::LoseMatchingWriteResponse(
        "INSERT INTO remote_gate",
    ));
    let error = store.acquire_gate("http.request").unwrap_err();
    assert_eq!(
        remote_kind(error.as_ref()),
        Some(meibo_db::RemoteErrorKind::OutcomeUnknown)
    );
    assert!(store.conn.is_poisoned());
    assert_eq!(proxy.triggered(), 1);
    let sent = proxy.requests();
    assert_gate(
        store.acquire_gate("http.request").unwrap_err(),
        GateError::UncertainConnection,
    );
    assert_gate(
        store.gate_status().unwrap_err(),
        GateError::UncertainConnection,
    );
    drop(store);
    assert_eq!(proxy.requests(), sent);
    let observed = direct.gate_status().unwrap().unwrap();
    assert_eq!(observed.operation, "http.request");
    assert_gate(
        direct.acquire_gate("cli.user-add").unwrap_err(),
        GateError::Busy,
    );
    direct.recover_gate(&observed, true).unwrap();
}

#[test]
fn poisoned_protected_connection_cannot_release_its_guard() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let direct = RemoteStore::initialize(endpoints(&server)).unwrap();
    let proxy = proxy::FaultProxy::start(server.address()).unwrap();
    let store = RemoteStore::open(endpoints_url(&proxy.endpoint())).unwrap();
    let guard = store.acquire_gate("http.request").unwrap();
    let observed = guard.status().clone();
    proxy.arm(proxy::Fault::LoseMatchingWriteResponse("INSERT INTO audit"));
    let error = store
        .audit("synthetic", None, "synthetic.action", "success")
        .unwrap_err();
    assert_eq!(
        remote_kind(error.as_ref()),
        Some(meibo_db::RemoteErrorKind::OutcomeUnknown)
    );
    let sent = proxy.requests();
    assert_gate(
        guard.release(&store).unwrap_err(),
        GateError::UncertainConnection,
    );
    drop(store);
    assert_eq!(proxy.requests(), sent);
    assert_eq!(direct.gate_status().unwrap().as_ref(), Some(&observed));
    direct.recover_gate(&observed, true).unwrap();
}

#[test]
fn operation_labels_are_bounded_and_never_echo_untrusted_content() {
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    let store = RemoteStore::initialize(endpoints(&server)).unwrap();
    for operation in [
        String::new(),
        "a".repeat(65),
        "synthetic-private-label/?secret=marker".into(),
        "label\nnext".into(),
    ] {
        let error = store.acquire_gate(&operation).unwrap_err();
        assert_eq!(
            error.downcast_ref::<GateError>(),
            Some(&GateError::InvalidOperation)
        );
        assert!(!error.to_string().contains("marker"));
    }
    assert!(store.gate_status().unwrap().is_none());
}

const CHILD: &str = "MEIBO_GATE_TEST_CHILD";
const PROCESS_TEST: &str =
    "hosted::remote_gate::tests::separate_processes_share_one_slot_and_crashed_owner_survives";

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "synthetic child did not reach its synchronization point"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn publish(path: &Path, value: &[u8]) {
    let pending = path.with_extension("pending");
    std::fs::write(&pending, value).unwrap();
    std::fs::rename(pending, path).unwrap();
}

#[test]
fn separate_processes_share_one_slot_and_crashed_owner_survives() {
    if let Ok(config) = std::env::var(CHILD) {
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let store = RemoteStore::open(endpoints_url(config["url"].as_str().unwrap())).unwrap();
        let ready = Path::new(config["ready"].as_str().unwrap());
        let barrier = Path::new(config["barrier"].as_str().unwrap());
        let result = Path::new(config["result"].as_str().unwrap());
        std::fs::write(ready, b"ready").unwrap();
        wait_file(barrier);
        match store.acquire_gate("http.request") {
            Ok(guard) => {
                publish(
                    result,
                    format!("owner:{}", guard.status().owner_hex()).as_bytes(),
                );
                loop {
                    thread::park();
                }
            }
            Err(error) => {
                assert_gate(error, GateError::Busy);
                publish(result, b"busy");
            }
        }
        return;
    }
    let _serial = serial();
    let server = sqld::Sqld::start().unwrap();
    drop(RemoteStore::initialize(endpoints(&server)).unwrap());
    let directory = tempfile::tempdir().unwrap();
    let barrier = directory.path().join("go");
    let mut children = Vec::new();
    for index in 0..2 {
        let config = serde_json::json!({"url":server.endpoint(),"ready":directory.path().join(format!("ready-{index}")),"barrier":barrier,"result":directory.path().join(format!("result-{index}"))});
        children.push(ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", PROCESS_TEST, "--nocapture", "--test-threads=1"])
                .env(CHILD, config.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        ));
    }
    for index in 0..2 {
        wait_file(&directory.path().join(format!("ready-{index}")));
    }
    std::fs::write(&barrier, b"go").unwrap();
    let mut outcomes = Vec::new();
    for index in 0..2 {
        let path = directory.path().join(format!("result-{index}"));
        wait_file(&path);
        outcomes.push(std::fs::read_to_string(path).unwrap());
    }
    assert_eq!(outcomes.iter().filter(|value| *value == "busy").count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| value.starts_with("owner:"))
            .count(),
        1
    );
    let owner = outcomes
        .iter()
        .find_map(|value| value.strip_prefix("owner:"))
        .unwrap();
    for child in &mut children {
        let _ = child.0.kill();
        let _ = child.0.wait();
    }
    let restarted = RemoteStore::open(endpoints(&server)).unwrap();
    let status = restarted.gate_status().unwrap().unwrap();
    assert_eq!(status.owner_hex(), owner);
    assert_gate(
        restarted.acquire_gate("cli.user-add").unwrap_err(),
        GateError::Busy,
    );
    restarted.recover_gate(&status, true).unwrap();
    restarted
        .acquire_gate("cli.user-add")
        .unwrap()
        .release(&restarted)
        .unwrap();
}
