//! Real loopback server/proxy lifecycle checks with synthetic data.
mod support;

use libsql_hrana::proto::Value;
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::Arc,
    time::{Duration, Instant},
};
use support::{
    client::Client,
    proxy::{Fault, FaultProxy},
    sqld::Sqld,
};

fn connect(endpoint: &str) -> Client {
    Client::connect(endpoint).unwrap()
}

fn scalar(connection: &mut Client, sql: &str) -> i64 {
    let result = connection.execute(sql).unwrap();
    match result.rows[0].values[0] {
        Value::Integer { value } => value,
        _ => panic!("expected a synthetic integer result"),
    }
}

#[test]
fn fixture_client_rejects_non_loopback_endpoints() {
    for endpoint in [
        "http://192.0.2.1:80",
        "https://127.0.0.1:443",
        "http://localhost:8080",
        "http://user@127.0.0.1:8080",
        "http://127.0.0.1:8080/?query=secret",
    ] {
        assert!(Client::connect(endpoint).is_err());
    }
}

#[test]
fn sqld_lifecycle_uses_an_isolated_database_and_reaps_the_child() {
    let server = Sqld::start().unwrap();
    let address = server.address();
    let directory = server.directory().to_owned();
    assert!(address.ip().is_loopback());
    assert!(directory.exists());
    let mut connection = connect(&server.endpoint());
    connection
        .sequence("CREATE TABLE synthetic(value INTEGER); INSERT INTO synthetic VALUES (42)")
        .unwrap();
    assert_eq!(scalar(&mut connection, "SELECT value FROM synthetic"), 42);
    drop(connection);
    drop(server);
    assert!(!directory.exists());
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());

    let another = Sqld::start().unwrap();
    assert_ne!(another.directory(), directory);
    assert_eq!(
        scalar(
            &mut connect(&another.endpoint()),
            "SELECT count(*) FROM sqlite_schema WHERE name='synthetic'"
        ),
        0
    );
}

#[test]
fn proxy_forwards_sql_and_can_delay_one_response() {
    let server = Sqld::start().unwrap();
    let proxy = FaultProxy::start(server.address()).unwrap();
    assert!(proxy.address().ip().is_loopback());
    let mut connection = connect(&proxy.endpoint());
    assert_eq!(scalar(&mut connection, "SELECT 42"), 42);
    proxy.arm(Fault::DelayResponse(Duration::from_millis(80)));
    let start = Instant::now();
    assert_eq!(scalar(&mut connection, "SELECT 43"), 43);
    assert!(start.elapsed() >= Duration::from_millis(80));
    assert_eq!(proxy.triggered(), 1);
    assert_eq!(scalar(&mut connection, "SELECT 44"), 44);
    assert_eq!(proxy.triggered(), 1);
    let address = proxy.address();
    drop(connection);
    drop(proxy);
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
}

#[test]
fn proxy_streams_an_oversized_error_body() {
    let server = Sqld::start().unwrap();
    let proxy = FaultProxy::start(server.address()).unwrap();
    let length = 2 * 1024 * 1024 + 17;
    proxy.arm(Fault::OversizedErrorBody(length));
    let mut stream = TcpStream::connect(proxy.address()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() <= 4096);
    }
    assert!(header.starts_with(b"HTTP/1.1 503"));
    let mut received = 0;
    let mut chunk = [0; 8192];
    loop {
        let read = stream.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        assert!(chunk[..read].iter().all(|&byte| byte == b'x'));
        received += read;
    }
    assert_eq!(received, length);
    assert_eq!(proxy.triggered(), 1);
}

#[test]
fn fixture_client_bounds_an_oversized_error_body() {
    let server = Sqld::start().unwrap();
    let proxy = FaultProxy::start(server.address()).unwrap();
    let mut connection = connect(&proxy.endpoint());
    proxy.arm(Fault::OversizedErrorBody(2 * 1024 * 1024 + 17));
    let error = connection.execute("SELECT 42").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(proxy.triggered(), 1);
}

#[test]
fn proxy_loses_the_commit_response_after_the_server_commits() {
    let server = Sqld::start().unwrap();
    let proxy = FaultProxy::start(server.address()).unwrap();
    let mut connection = connect(&proxy.endpoint());
    connection
        .execute("CREATE TABLE synthetic(value INTEGER)")
        .unwrap();
    connection.execute("BEGIN IMMEDIATE").unwrap();
    connection
        .execute("INSERT INTO synthetic VALUES (91)")
        .unwrap();
    proxy.arm(Fault::LoseCommitResponse);
    assert_eq!(scalar(&mut connection, "SELECT value FROM synthetic"), 91);
    assert_eq!(proxy.triggered(), 0);
    let result = connection.execute("COMMIT");
    assert!(result.is_err());
    assert_eq!(proxy.triggered(), 1);
    assert_eq!(
        scalar(
            &mut connect(&server.endpoint()),
            "SELECT value FROM synthetic"
        ),
        91
    );
}

#[test]
fn sqld_child_is_cleaned_up_when_the_owner_panics() {
    let result = Arc::new(std::sync::Mutex::new(None));
    let captured = result.clone();
    let panic = std::panic::catch_unwind(move || {
        let server = Sqld::start().unwrap();
        *captured.lock().unwrap() = Some((server.address(), server.directory().to_owned()));
        panic!("synthetic fixture owner panic");
    });
    assert!(panic.is_err());
    let (address, directory) = result.lock().unwrap().take().unwrap();
    assert!(!directory.exists());
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
}
