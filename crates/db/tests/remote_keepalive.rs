//! Connection reuse and the read fence (remote-performance 4.1 and 4.3).
//!
//! Requirement 6.3: before reads may reuse a socket, show for three faults
//! that the database receives each round trip at most once, and that every
//! round trip that may change data uses a new socket. Only round trips made
//! inside a [`ReadFence`] may reuse a socket; outside it every one is new.
#[allow(dead_code)] // Shared fixtures expose operations used by other binaries.
mod support;

use meibo_db::{
    Connection, REUSE_READ_CONNECTIONS, ReadFence, RemoteOptions, TransactionBehavior,
    remote_error_kind,
};
use std::{collections::BTreeMap, time::Duration};
use support::{
    client::Client,
    keepalive::{Fault, KeepAliveProxy, Received},
    proxy::FaultProxy,
    sqld::Sqld,
};

static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

macro_rules! keepalive_test {
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

fn database() -> Sqld {
    let server = Sqld::start().unwrap();
    Client::connect(&server.endpoint())
        .unwrap()
        .sequence("CREATE TABLE synthetic(id INTEGER PRIMARY KEY, value INTEGER)")
        .unwrap();
    server
}

fn connect(endpoint: &str) -> Connection {
    Connection::open_remote(
        RemoteOptions::new(endpoint, "synthetic-keepalive-token").allow_loopback_http_for_tests(),
    )
    .unwrap()
}

/// Each distinct body reached the database at most once. The foreign-key
/// setup of the write path is identical for every write and always travels
/// on a new socket, so it is checked by position instead.
fn at_most_once(received: &[Received]) {
    let mut seen = BTreeMap::<&[u8], usize>::new();
    for request in received {
        if String::from_utf8_lossy(&request.body).contains("PRAGMA foreign_keys") {
            assert_eq!(request.position, 0);
            continue;
        }
        *seen.entry(request.body.as_slice()).or_default() += 1;
    }
    assert!(
        seen.values().all(|count| *count == 1),
        "a round trip reached the database more than once"
    );
}

/// A round trip that may change data: any transaction stage (it carries a
/// baton or opens a stream) or a statement that is not a plain SELECT.
/// Describe never executes SQL.
fn may_write(body: &[u8]) -> bool {
    use libsql_hrana::proto::{PipelineReqBody, StreamRequest};
    let body: PipelineReqBody = serde_json::from_slice(body).unwrap();
    let closing = matches!(body.requests.last(), Some(StreamRequest::Close(_)));
    body.baton.is_some()
        || !closing
        || body.requests.iter().any(|request| match request {
            StreamRequest::Execute(request) => !request
                .stmt
                .sql
                .as_deref()
                .is_some_and(|sql| sql.starts_with("SELECT")),
            StreamRequest::Sequence(_) | StreamRequest::Batch(_) => true,
            _ => false,
        })
}

/// Distinct SELECT texts so that every round trip has a distinct body.
fn read(connection: &Connection, value: i64) -> meibo_db::Result<i64> {
    connection.query_row(&format!("SELECT {value}"), [], |row| row.get(0))
}

keepalive_test!(reads_reuse_one_socket_and_writes_always_use_new_ones, {
    let server = database();
    let proxy = KeepAliveProxy::start(server.address(), None).unwrap();
    let mut connection = connect(&proxy.endpoint());
    // Outside a fence (serialized path, CLI): every round trip is fresh.
    for value in 1..=3 {
        assert_eq!(read(&connection, value).unwrap(), value);
    }
    assert_eq!(
        proxy.sockets(),
        6,
        "reads outside a fence never reuse a socket"
    );
    {
        let _fence = ReadFence::begin();
        for value in 4..=6 {
            assert_eq!(read(&connection, value).unwrap(), value);
        }
    }
    if REUSE_READ_CONNECTIONS {
        assert_eq!(proxy.sockets(), 7, "fenced reads share one socket");
    } else {
        assert_eq!(proxy.sockets(), 12);
    }
    connection
        .execute("INSERT INTO synthetic(value) VALUES(?1)", [7])
        .unwrap();
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute("INSERT INTO synthetic(value) VALUES(?1)", [8])
        .unwrap();
    tx.commit().unwrap();
    let received = proxy.received();
    at_most_once(&received);
    for request in received.iter().filter(|request| may_write(&request.body)) {
        assert_eq!(
            request.position, 0,
            "a data-changing round trip reused a socket"
        );
    }
    assert!(!connection.is_poisoned());
});

keepalive_test!(lost_reply_on_a_reused_socket_is_not_resent, {
    let server = database();
    let proxy = KeepAliveProxy::start(server.address(), Some(Fault::DropBeforeResponse)).unwrap();
    let connection = connect(&proxy.endpoint());
    // The first round trip opens the socket; the next one reuses it.
    let _fence = ReadFence::begin();
    let first = read(&connection, 11);
    if REUSE_READ_CONNECTIONS {
        assert!(first.is_err());
        assert_eq!(proxy.triggered(), 1);
        assert!(connection.is_poisoned());
        let before = proxy.received().len();
        assert!(read(&connection, 12).is_err());
        assert_eq!(
            proxy.received().len(),
            before,
            "a poisoned connection sent again"
        );
    } else {
        assert_eq!(first.unwrap(), 11);
        assert_eq!(proxy.triggered(), 0);
    }
    at_most_once(&proxy.received());
    assert_eq!(proxy.forwarded(), proxy.received().len());
});

keepalive_test!(request_received_then_closed_is_not_resent, {
    let server = database();
    let proxy = KeepAliveProxy::start(server.address(), Some(Fault::CloseAfterReceive)).unwrap();
    let connection = connect(&proxy.endpoint());
    let _fence = ReadFence::begin();
    let first = read(&connection, 21);
    if REUSE_READ_CONNECTIONS {
        assert!(first.is_err());
        assert_eq!(proxy.triggered(), 1);
        assert!(connection.is_poisoned());
        assert!(
            remote_error_kind(&first.unwrap_err()).is_none(),
            "a read failure keeps its sanitized transport error"
        );
    } else {
        assert_eq!(first.unwrap(), 21);
    }
    at_most_once(&proxy.received());
});

keepalive_test!(
    database_closing_an_idle_socket_never_duplicates_a_round_trip,
    {
        let server = database();
        // Close before, around and after the moment the next request is sent.
        for (delay, pause) in [(0, 30), (5, 5), (20, 0), (1, 1), (0, 0)] {
            let proxy = KeepAliveProxy::start(
                server.address(),
                Some(Fault::CloseIdleAfter(Duration::from_millis(delay))),
            )
            .unwrap();
            let connection = connect(&proxy.endpoint());
            let _fence = ReadFence::begin();
            for value in 0..8 {
                std::thread::sleep(Duration::from_millis(pause));
                if read(&connection, 100 + value).is_err() {
                    // A failed read poisons the connection; it is never retried.
                    assert!(connection.is_poisoned());
                    break;
                }
            }
            at_most_once(&proxy.received());
            assert!(proxy.received().len() <= 16);
        }
    }
);

keepalive_test!(
    read_fence_refuses_writes_before_sending_and_always_restores,
    {
        let server = database();
        let proxy = FaultProxy::start(server.address()).unwrap();
        let mut connection = connect(&proxy.endpoint());
        connection
            .execute("INSERT INTO synthetic(value) VALUES(?1)", [1])
            .unwrap();
        {
            let fence = ReadFence::begin();
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM synthetic", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert!(!fence.tripped());
            let tx = connection.unchecked_transaction().unwrap();
            assert_eq!(
                tx.query_row("SELECT value FROM synthetic", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            tx.commit().unwrap();
            assert!(!fence.tripped());
            let sent = proxy.requests();
            // Already described above: refusals send nothing at all.
            let refused = connection.execute("INSERT INTO synthetic(value) VALUES(?1)", [2]);
            assert!(refused.is_err());
            assert_eq!(proxy.requests(), sent);
            assert!(fence.tripped());
            assert!(connection.execute_batch("DELETE FROM synthetic").is_err());
            assert!(
                connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .is_err()
            );
            assert_eq!(proxy.requests(), sent);
            assert!(!connection.is_poisoned());
            assert!(
                !refused
                    .as_ref()
                    .err()
                    .and_then(remote_error_kind)
                    .is_some_and(|kind| kind == meibo_db::RemoteErrorKind::OutcomeUnknown),
                "a refusal is not an uncertain outcome"
            );
            // A connection opened inside the fence is fenced as well.
            let later = connect(&proxy.endpoint());
            assert!(
                later
                    .execute("INSERT INTO synthetic(value) VALUES(3)", [])
                    .is_err()
            );
        }
        connection
            .execute("INSERT INTO synthetic(value) VALUES(?1)", [4])
            .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM synthetic", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
);
