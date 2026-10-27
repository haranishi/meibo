//! Same-stream transactions against real synthetic sqld and a fault proxy.
#[allow(dead_code)] // Shared fixtures expose operations used by other binaries.
mod support;

use meibo_db::{
    Connection, Error, ErrorCode, RemoteErrorKind, RemoteOptions, TransactionBehavior,
    remote_error_kind,
};
use std::time::Duration;
use support::{
    client::Client,
    proxy::{Fault, FaultProxy},
    sqld::Sqld,
};

static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

macro_rules! transaction_test {
    ($name:ident, $body:block) => {
        #[test]
        fn $name() {
            let _fixture = FIXTURES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            $body
        }
    };
}

fn options(endpoint: &str) -> RemoteOptions {
    RemoteOptions::new(endpoint, "synthetic-transaction-token").allow_loopback_http_for_tests()
}

fn database() -> Sqld {
    let server = Sqld::start().unwrap();
    assert!(server.directory().is_dir());
    Client::connect(&server.endpoint()).unwrap().sequence(
        "CREATE TABLE synthetic(id INTEGER PRIMARY KEY, value INTEGER UNIQUE); CREATE TABLE events(value TEXT)",
    ).unwrap();
    server
}

fn connect(endpoint: &str) -> Connection {
    Connection::open_remote(options(endpoint)).unwrap()
}

fn count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT count(*) FROM synthetic", [], |row| row.get(0))
        .unwrap()
}

fn observed_count(server: &Sqld) -> i64 {
    let result = Client::connect(&server.endpoint())
        .unwrap()
        .execute("SELECT count(*) FROM synthetic")
        .unwrap();
    match result.rows[0].values[0] {
        libsql_hrana::proto::Value::Integer { value } => value,
        _ => panic!("expected synthetic row count"),
    }
}

fn private(error: &Error, endpoint: &str) {
    let output = format!("{error} {error:?}");
    for secret in [
        endpoint,
        "synthetic-transaction-token",
        "synthetic_sql_secret",
    ] {
        assert!(!output.contains(secret));
    }
}

transaction_test!(provider_style_stateless_setup_keeps_real_query_streams, {
    let server = database();
    let proxy = FaultProxy::start(server.address()).unwrap();
    proxy.arm(Fault::EndUnstoredStream);
    let connection = connect(&proxy.endpoint());
    let actual: i64 = connection
        .query_row("SELECT 17", [], |row| row.get(0))
        .unwrap();
    assert_eq!(actual, 17);
    assert_eq!(
        connection
            .execute("INSERT INTO synthetic(value) VALUES(41)", [])
            .unwrap(),
        1
    );
    assert_eq!(connection.last_insert_rowid(), 1);
    let tx = connection.unchecked_transaction().unwrap();
    assert_eq!(
        connection
            .execute("INSERT INTO synthetic(value) VALUES(42)", [])
            .unwrap(),
        1
    );
    tx.commit().unwrap();
    assert_eq!(count(&connection), 2);
    assert_eq!(connection.last_insert_rowid(), 2);
    assert_eq!(proxy.triggered(), 0);
    assert!(proxy.fresh_streams() > 0);
    assert_eq!(proxy.stored_sql_uses(), proxy.fresh_streams());
    assert!(connection.is_autocommit());
    assert!(!connection.is_poisoned());
});

transaction_test!(provider_style_unstored_control_stream_is_really_closed, {
    let server = database();
    let proxy = FaultProxy::start(server.address()).unwrap();
    proxy.arm(Fault::EndUnstoredStream);
    let result = Client::connect(&proxy.endpoint())
        .unwrap()
        .execute("SELECT 17")
        .unwrap();
    assert!(matches!(
        result.rows[0].values[0],
        libsql_hrana::proto::Value::Integer { value: 17 }
    ));
    assert_eq!(proxy.triggered(), 1);
    assert_eq!(proxy.requests(), 1);
});

transaction_test!(
    original_connection_and_prepared_statement_share_transaction,
    {
        let server = database();
        let connection = connect(&server.endpoint());
        assert!(connection.is_remote());
        assert!(!connection.is_poisoned());
        assert!(connection.is_autocommit());
        let mut statement = connection
            .prepare_cached("INSERT INTO synthetic(value) VALUES(?1)")
            .unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        assert!(!connection.is_autocommit());
        statement.execute([41]).unwrap();
        assert_eq!(transaction.last_insert_rowid(), 1);
        assert_eq!(count(&connection), 1);
        assert_eq!(observed_count(&server), 0);
        assert!(connection.unchecked_transaction().is_err());
        transaction.commit().unwrap();
        assert!(connection.is_autocommit());
        assert_eq!(observed_count(&server), 1);
        assert_eq!(connection.last_insert_rowid(), 1);
    }
);

transaction_test!(
    explicit_rollback_drop_and_callback_failure_undo_all_changes,
    {
        let server = database();
        let connection = connect(&server.endpoint());
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute("INSERT INTO synthetic(value) VALUES(1)", [])
            .unwrap();
        transaction.rollback().unwrap();
        assert_eq!(count(&connection), 0);
        {
            let transaction = connection.unchecked_transaction().unwrap();
            transaction
                .execute("INSERT INTO synthetic(value) VALUES(2)", [])
                .unwrap();
        }
        assert_eq!(count(&connection), 0);
        let outcome: meibo_db::Result<()> = (|| {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute("INSERT INTO synthetic(value) VALUES(3)", [])?;
            let error = transaction
                .execute("INSERT INTO synthetic(value) VALUES(3)", [])
                .unwrap_err();
            assert_eq!(
                error.sqlite_error_code(),
                Some(ErrorCode::ConstraintViolation)
            );
            Err(error)
        })();
        assert!(outcome.is_err());
        assert_eq!(observed_count(&server), 0);
        assert!(connection.is_autocommit());
        assert!(!connection.is_poisoned());
    }
);

transaction_test!(
    transaction_behaviors_and_same_stream_savepoints_are_supported,
    {
        let server = database();
        let mut connection = connect(&server.endpoint());
        for (value, behavior) in [
            (1, TransactionBehavior::Deferred),
            (2, TransactionBehavior::Immediate),
            (3, TransactionBehavior::Exclusive),
        ] {
            let transaction = connection.transaction_with_behavior(behavior).unwrap();
            transaction
                .execute("INSERT INTO synthetic(value) VALUES(?1)", [value])
                .unwrap();
            transaction.execute_batch("SAVEPOINT meibo_recount; INSERT INTO synthetic(value) VALUES(99); ROLLBACK TO meibo_recount; RELEASE meibo_recount").unwrap();
            transaction.commit().unwrap();
        }
        assert_eq!(count(&connection), 3);
        let transaction = connection.transaction().unwrap();
        assert!(!transaction.is_autocommit());
        transaction.rollback().unwrap();
    }
);

transaction_test!(
    deferred_constraint_commit_failure_rolls_back_and_reuses_connection,
    {
        let server = database();
        let mut connection = connect(&server.endpoint());
        connection.execute_batch("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED)").unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("INSERT INTO child VALUES(7)", [])
            .unwrap();
        let error = transaction.commit().unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(ErrorCode::ConstraintViolation)
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM child", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(connection.is_autocommit());
        assert!(!connection.is_poisoned());
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("INSERT INTO parent VALUES(7)", [])
            .unwrap();
        transaction.commit().unwrap();
    }
);

transaction_test!(
    sequence_preserves_triggers_quotes_comments_and_write_identity,
    {
        let server = database();
        let connection = connect(&server.endpoint());
        let transaction = connection.unchecked_transaction().unwrap();
        transaction.execute_batch("CREATE TRIGGER synthetic_ai AFTER INSERT ON synthetic BEGIN INSERT INTO events VALUES('COMMIT; BEGIN;'); INSERT INTO events VALUES(CASE WHEN new.id=0 THEN 'zero' ELSE 'other;END' END); END; /* BEGIN; */ INSERT INTO synthetic(id,value) VALUES(0,7); -- COMMIT\n").unwrap();
        assert_eq!(transaction.last_insert_rowid(), 0);
        transaction
            .execute(
                "INSERT INTO synthetic(id,value) VALUES(?1,?2)",
                (i64::MAX, 8),
            )
            .unwrap();
        assert_eq!(transaction.last_insert_rowid(), i64::MAX);
        assert_eq!(
            transaction
                .query_row("SELECT count(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            4
        );
        transaction
            .execute("UPDATE synthetic SET value=9 WHERE id=0", [])
            .unwrap();
        assert_eq!(transaction.last_insert_rowid(), i64::MAX);
        transaction.commit().unwrap();
        assert_eq!(connection.last_insert_rowid(), i64::MAX);
        assert_eq!(count(&connection), 2);
        connection
            .execute("UPDATE synthetic SET value=10 WHERE id=0", [])
            .unwrap();
        assert_eq!(connection.last_insert_rowid(), i64::MAX);
    }
);

transaction_test!(raw_outer_transaction_controls_reject_before_any_request, {
    let server = database();
    let proxy = FaultProxy::start(server.address()).unwrap();
    let connection = connect(&proxy.endpoint());
    for sql in [
        "BEGIN",
        "-- comment\n BEGIN IMMEDIATE",
        "COMMIT",
        "END TRANSACTION",
        "ROLLBACK",
        "SAVEPOINT foo",
        "RELEASE foo",
        "ROLLBACK TO foo",
        "INSERT INTO synthetic VALUES(1,1); COMMIT;",
        "CREATE TRIGGER synthetic_ai AFTER INSERT ON synthetic BEGIN INSERT INTO events VALUES('semi;colon'); END; COMMIT",
    ] {
        let before = proxy.requests();
        let error = connection.execute_batch(sql).unwrap_err();
        assert_eq!(
            remote_error_kind(&error),
            Some(RemoteErrorKind::Unsupported)
        );
        assert_eq!(proxy.requests(), before);
    }
    let transaction = connection.unchecked_transaction().unwrap();
    for sql in ["COMMIT", "BEGIN", "ROLLBACK", "END"] {
        let before = proxy.requests();
        assert_eq!(
            remote_error_kind(&connection.execute(sql, []).unwrap_err()),
            Some(RemoteErrorKind::Unsupported)
        );
        assert_eq!(proxy.requests(), before);
    }
    transaction.rollback().unwrap();
    assert_eq!(observed_count(&server), 0);
    assert!(!connection.is_poisoned());
});

transaction_test!(
    commit_response_loss_is_unknown_without_retry_or_drop_rollback,
    {
        let server = database();
        let proxy = FaultProxy::start(server.address()).unwrap();
        let connection = connect(&proxy.endpoint());
        let mut statement = connection.prepare("SELECT value FROM synthetic").unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute("INSERT INTO synthetic VALUES(1,17)", [])
            .unwrap();
        proxy.arm(Fault::LoseCommitResponse);
        let error = transaction.commit().unwrap_err();
        assert_eq!(
            remote_error_kind(&error),
            Some(RemoteErrorKind::OutcomeUnknown)
        );
        private(&error, &proxy.endpoint());
        assert!(connection.is_poisoned());
        assert!(!connection.is_autocommit());
        assert_eq!(proxy.triggered(), 1);
        assert_eq!(proxy.commits(), 1);
        assert_eq!(proxy.rollbacks(), 0);
        let sent = proxy.requests();
        assert_eq!(
            remote_error_kind(
                &connection
                    .execute("INSERT INTO synthetic VALUES(2,18)", [])
                    .unwrap_err()
            ),
            Some(RemoteErrorKind::ConnectionPoisoned)
        );
        assert!(
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .is_err()
        );
        assert!(connection.prepare("SELECT 1").is_err());
        assert!(connection.execute_batch("SELECT 1").is_err());
        assert!(connection.unchecked_transaction().is_err());
        assert!(statement.query([]).is_err());
        assert_eq!(proxy.requests(), sent);
        assert_eq!(observed_count(&server), 1);
    }
);

transaction_test!(drop_rollback_failure_is_observable_and_never_retried, {
    let server = database();
    let proxy = FaultProxy::start(server.address()).unwrap();
    let connection = connect(&proxy.endpoint());
    {
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute("INSERT INTO synthetic VALUES(1,19)", [])
            .unwrap();
        proxy.arm(Fault::LoseRollbackResponse);
    }
    assert!(connection.is_poisoned());
    assert_eq!(proxy.rollbacks(), 1);
    assert_eq!(proxy.commits(), 0);
    assert_eq!(proxy.triggered(), 1);
    let sent = proxy.requests();
    assert!(connection.execute_batch("SELECT 1").is_err());
    assert_eq!(proxy.requests(), sent);
    assert_eq!(observed_count(&server), 0);
});

transaction_test!(exact_once_known_commit_and_rollback_cleanup, {
    let server = database();
    let proxy = FaultProxy::start(server.address()).unwrap();
    let connection = connect(&proxy.endpoint());
    connection
        .unchecked_transaction()
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!((proxy.commits(), proxy.rollbacks()), (1, 0));
    connection
        .unchecked_transaction()
        .unwrap()
        .rollback()
        .unwrap();
    assert_eq!((proxy.commits(), proxy.rollbacks()), (1, 1));
    drop(connection.unchecked_transaction().unwrap());
    assert_eq!((proxy.commits(), proxy.rollbacks()), (1, 2));
    assert!(!connection.is_poisoned());
});

transaction_test!(
    same_origin_route_is_retained_when_later_responses_omit_base_url,
    {
        let server = database();
        let proxy = FaultProxy::start(server.address()).unwrap();
        let connection = connect(&proxy.endpoint());
        proxy.arm(Fault::RoutePrefix);
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute("INSERT INTO synthetic VALUES(1,20)", [])
            .unwrap();
        assert_eq!(count(&connection), 1);
        transaction.commit().unwrap();
        assert_eq!(observed_count(&server), 1);
        assert_eq!(proxy.triggered(), 1);
    }
);

transaction_test!(transport_and_size_failures_poison_the_shared_backend, {
    for fault in [
        Fault::DelayResponse(Duration::from_millis(100)),
        Fault::OversizedErrorBody(1025),
    ] {
        let server = database();
        let proxy = FaultProxy::start(server.address()).unwrap();
        let options = options(&proxy.endpoint())
            .with_timeout(Duration::from_millis(60))
            .unwrap()
            .with_response_limit(1024)
            .unwrap();
        let connection = Connection::open_remote(options).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        proxy.arm(fault);
        let error = transaction
            .execute("INSERT INTO synthetic VALUES(1,21)", [])
            .unwrap_err();
        assert_eq!(
            remote_error_kind(&error),
            Some(RemoteErrorKind::OutcomeUnknown)
        );
        private(&error, &proxy.endpoint());
        drop(transaction);
        assert!(connection.is_poisoned());
        assert_eq!(proxy.triggered(), 1);
        assert_eq!(proxy.rollbacks(), 0);
        let sent = proxy.requests();
        assert!(connection.execute_batch("SELECT 1").is_err());
        assert_eq!(proxy.requests(), sent);
    }
});

transaction_test!(
    local_backend_remains_send_and_keeps_native_file_operations,
    {
        fn assert_send<T: Send>() {}
        assert_send::<Connection>();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic-local.db");
        let connection = Connection::open(&path).unwrap();
        assert!(!connection.is_remote());
        assert!(!connection.is_poisoned());
        std::thread::spawn(move || {
            connection
                .execute_batch("CREATE TABLE synthetic(value); INSERT INTO synthetic VALUES(29)")
                .unwrap();
            connection
                .query_row("SELECT value FROM synthetic", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        })
        .join()
        .map(|value| assert_eq!(value, 29))
        .unwrap();
        let server = database();
        let mut remote = connect(&server.endpoint());
        assert!(remote.path().is_none());
        assert_eq!(
            remote_error_kind(&remote.busy_timeout(Duration::from_secs(1)).unwrap_err()),
            Some(RemoteErrorKind::Unsupported)
        );
        assert_eq!(
            remote_error_kind(
                &remote
                    .pragma_update(None, "foreign_keys", true)
                    .unwrap_err()
            ),
            Some(RemoteErrorKind::Unsupported)
        );
        assert_eq!(
            remote_error_kind(
                &remote
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap_err()
            ),
            Some(RemoteErrorKind::Unsupported)
        );
        assert_eq!(
            remote_error_kind(
                &remote
                    .backup(meibo_db::MAIN_DB, directory.path().join("backup.db"), None)
                    .unwrap_err()
            ),
            Some(RemoteErrorKind::Unsupported)
        );
        assert_eq!(
            remote_error_kind(
                &remote
                    .restore(
                        meibo_db::MAIN_DB,
                        &path,
                        None::<fn(meibo_db::backup::Progress)>
                    )
                    .unwrap_err()
            ),
            Some(RemoteErrorKind::Unsupported)
        );
        assert!(!directory.path().join("backup.db").exists());
    }
);

#[test]
fn only_reserved_adapter_messages_and_codes_classify_as_adapter_outcomes() {
    let provider_error = Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some("remote database outcome unknown".into()),
    );
    assert_eq!(remote_error_kind(&provider_error), None);
    let other_message = Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
        Some("remote database statement failed".into()),
    );
    assert_eq!(remote_error_kind(&other_message), None);
}

transaction_test!(autocommit_write_response_loss_is_explicitly_unknown, {
    for sequence in [false, true] {
        let server = database();
        let proxy = FaultProxy::start(server.address()).unwrap();
        let connection = connect(&proxy.endpoint());
        let mut insert = connection
            .prepare("INSERT INTO synthetic VALUES(1,31)")
            .unwrap();
        proxy.arm(Fault::LoseWriteResponse);
        let error = if sequence {
            connection
                .execute_batch("INSERT INTO synthetic VALUES(1,31)")
                .unwrap_err()
        } else {
            insert.execute([]).unwrap_err()
        };
        assert_eq!(observed_count(&server), 1);
        assert_eq!(proxy.triggered(), 1);
        assert_eq!(
            remote_error_kind(&error),
            Some(RemoteErrorKind::OutcomeUnknown)
        );
        private(&error, &proxy.endpoint());
        assert!(connection.is_poisoned());
        let sent = proxy.requests();
        assert!(
            connection
                .execute_batch("INSERT INTO synthetic VALUES(2,32)")
                .is_err()
        );
        assert_eq!(proxy.requests(), sent);
        assert_eq!(proxy.rollbacks(), 0);
    }
});
