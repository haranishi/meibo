//! Private write-policy fault coverage, absent from production and default tests.

#![cfg(all(test, feature = "integration-tests"))]

use std::{sync::Mutex, time::Duration};

use crate::{Engine, RemoteIdentity};
use meibo_db::{RemoteErrorKind, RemoteOptions, remote_error_kind};

use super::{DataOrigin, EngineError, OpenOptions, WRITE_BASE, storage, with_erase_gate};

#[allow(dead_code)]
#[path = "../../../db/tests/support/client.rs"]
mod client;
#[allow(dead_code)]
#[path = "../../../db/tests/support/proxy.rs"]
mod proxy;
#[allow(dead_code)]
#[path = "../../../db/tests/support/sqld.rs"]
mod sqld;

static FIXTURES: Mutex<()> = Mutex::new(());

fn callback_loss(erase_gate: bool) {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = sqld::Sqld::start().unwrap();
    let identity = RemoteIdentity::new("tenant-private-write", "crm").unwrap();
    let endpoint = RemoteOptions::new(server.endpoint(), "synthetic-private-token")
        .allow_loopback_http_for_tests();
    drop(
        Engine::create_remote(
            endpoint,
            &identity,
            DataOrigin::Synthetic,
            &OpenOptions::default(),
        )
        .unwrap(),
    );
    let proxy = proxy::FaultProxy::start(server.address()).unwrap();
    let endpoint = RemoteOptions::new(proxy.endpoint(), "synthetic-private-token")
        .allow_loopback_http_for_tests()
        .with_timeout(Duration::from_millis(500))
        .unwrap();
    let mut engine = Engine::open_remote(endpoint, &identity, &OpenOptions::default()).unwrap();
    assert_eq!(
        engine
            .db
            .local_growth_estimate(|| panic!("local growth estimator ran remotely"))
            .unwrap(),
        0
    );
    let rollbacks = proxy.rollbacks();
    let commits = proxy.commits();
    let error = engine.db.write(WRITE_BASE, |tx| {
        let lose_write = |tx: &super::Transaction<'_>| {
            let mut statement = tx.prepare("INSERT INTO companies(name,name_key,prefecture_code,created_at,updated_at) VALUES('synthetic','synthetic',13,1,1)").map_err(storage)?;
            // Arm only after BEGIN, optional erase gate insertion and Describe.
            // The next request is the actual write, which sqld receives.
            proxy.arm(proxy::Fault::DelayResponse(Duration::from_millis(1500)));
            statement.execute([]).map_err(storage)?;
            Ok(())
        };
        if erase_gate { with_erase_gate(tx, lose_write) } else { lose_write(tx) }
    }).unwrap_err();
    let EngineError::Storage(source) = error else {
        panic!("transport loss did not preserve its storage cause")
    };
    assert_eq!(
        source
            .downcast_ref::<meibo_db::Error>()
            .and_then(remote_error_kind),
        Some(RemoteErrorKind::OutcomeUnknown)
    );
    assert!(engine.is_poisoned());
    assert_eq!(proxy.triggered(), 1);
    assert_eq!(proxy.rollbacks(), rollbacks);
    assert_eq!(proxy.commits(), commits);
    let sent = proxy.requests();
    let next = engine
        .db
        .write::<()>(WRITE_BASE, |_| panic!("poisoned connection ran a callback"))
        .unwrap_err();
    let EngineError::Storage(source) = next else {
        panic!("poison was not a storage failure")
    };
    assert_eq!(
        source
            .downcast_ref::<meibo_db::Error>()
            .and_then(remote_error_kind),
        Some(RemoteErrorKind::ConnectionPoisoned)
    );
    drop(engine);
    assert_eq!(proxy.requests(), sent);
}

#[test]
fn lost_write_inside_callback_preserves_outcome_without_rollback() {
    callback_loss(false);
}

#[test]
fn lost_write_inside_erase_gate_preserves_outcome_without_gate_cleanup() {
    callback_loss(true);
}
