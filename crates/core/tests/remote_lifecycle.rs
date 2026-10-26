//! Real, explicitly enabled remote CRM lifecycle acceptance tests.

#![cfg(feature = "integration-tests")]

use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use meibo_core::{
    Engine, Integrity, OpenOptions, RemoteIdentity, SpaceProbe,
    types::{DataOrigin, EngineError, Filters},
};
use meibo_db::{Connection, RemoteOptions};

#[allow(dead_code)]
#[path = "../../db/tests/support/sqld.rs"]
mod sqld;

static FIXTURES: Mutex<()> = Mutex::new(());

struct NoFilesystem;
impl SpaceProbe for NoFilesystem {
    fn available(&self, _: &std::path::Path) -> io::Result<u64> {
        panic!("remote lifecycle must not inspect a local filesystem")
    }
}

fn options() -> OpenOptions {
    OpenOptions {
        space_probe: Some(Arc::new(NoFilesystem)),
        // Deliberately true: remote constructors must not start this worker.
        background_checkpoint: true,
        ..OpenOptions::default()
    }
}

fn endpoint(server: &sqld::Sqld) -> RemoteOptions {
    RemoteOptions::new(server.endpoint(), "synthetic-core-token").allow_loopback_http_for_tests()
}

fn identity() -> RemoteIdentity {
    RemoteIdentity::new("tenant-alpha", "crm").unwrap()
}

fn with_server(run: impl FnOnce(sqld::Sqld, Connection)) {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = sqld::Sqld::start().unwrap();
    let conn = Connection::open_remote(endpoint(&server)).unwrap();
    run(server, conn);
}

fn rejected(result: Result<Engine, EngineError>) -> EngineError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("invalid remote lifecycle input was accepted"),
    }
}

fn meta(conn: &Connection, key: &str) -> String {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
        .unwrap()
}

type SchemaEntry = (String, String, Option<String>);

fn schema(conn: &Connection) -> Vec<SchemaEntry> {
    conn.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )
    .unwrap()
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    .unwrap()
    .collect::<meibo_db::Result<_>>()
    .unwrap()
}

#[test]
fn empty_database_gets_shared_schema_and_explicit_remote_metadata_without_file_policy() {
    with_server(|server, conn| {
        let engine = Engine::create_remote(
            endpoint(&server),
            &identity(),
            DataOrigin::Synthetic,
            &options(),
        )
        .unwrap();
        assert_eq!(engine.origin(), DataOrigin::Synthetic);
        assert!(engine.path().is_none());
        assert!(!engine.is_poisoned());
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 0);
        for (key, expected) in [
            ("remote_format", "meibo-remote-v1"),
            ("schema_version", "1"),
            ("database_id", "tenant-alpha"),
            ("database_kind", "crm"),
            ("data_origin", "synthetic"),
            ("crm_schema_version", "1"),
            ("custom_fields_schema_version", "1"),
            ("public_data_schema_version", "1"),
        ] {
            assert_eq!(meta(&conn, key), expected);
        }
        assert!(meta(&conn, "created_at").parse::<u64>().is_ok());
        for pragma in ["PRAGMA application_id", "PRAGMA user_version"] {
            let value: i64 = conn.query_row(pragma, [], |r| r.get(0)).unwrap();
            assert_eq!(value, 0, "remote must not write SQLite header pragmas");
        }
        for name in [
            "meta",
            "companies",
            "contacts",
            "activities",
            "lists",
            "list_members",
            "counts",
            "erase_gate",
            "name_fts",
            "crm_work_items",
            "crm_opportunities",
            "custom_field_definitions",
            "custom_field_values",
            "erased_corporate_numbers",
        ] {
            assert!(schema(&conn).iter().any(|(_, found, _)| found == name));
        }
    });
}

#[test]
fn both_origins_and_existing_content_survive_a_new_engine_instance() {
    for origin in [DataOrigin::Synthetic, DataOrigin::Real] {
        with_server(|server, conn| {
            let engine =
                Engine::create_remote(endpoint(&server), &identity(), origin, &options()).unwrap();
            drop(engine);
            conn.execute(
                "INSERT INTO lists(id,name,created_at) VALUES(7,'合成保存リスト',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meta(key,value) VALUES('opaque_test_data','DROP TABLE companies;')",
                [],
            )
            .unwrap();
            let before = schema(&conn);
            for integrity in [Integrity::Auto, Integrity::Always, Integrity::Never] {
                let opt = OpenOptions {
                    integrity,
                    ..options()
                };
                let reopened = Engine::open_remote(endpoint(&server), &identity(), &opt).unwrap();
                assert_eq!(reopened.origin(), origin);
                assert!(reopened.path().is_none());
                assert!(!reopened.is_poisoned());
            }
            let name: String = conn
                .query_row("SELECT name FROM lists WHERE id=7", [], |r| r.get(0))
                .unwrap();
            assert_eq!(name, "合成保存リスト");
            assert_eq!(schema(&conn), before);
            assert_eq!(meta(&conn, "opaque_test_data"), "DROP TABLE companies;");
        });
    }
}

#[test]
fn existing_foreign_content_is_refused_without_overwrite() {
    with_server(|server, conn| {
        conn.execute_batch(
            "CREATE TABLE foreign_data(value TEXT);INSERT INTO foreign_data VALUES('retained');",
        )
        .unwrap();
        let before = schema(&conn);
        rejected(Engine::create_remote(
            endpoint(&server),
            &identity(),
            DataOrigin::Synthetic,
            &options(),
        ));
        assert_eq!(schema(&conn), before);
        assert_eq!(
            conn.query_row("SELECT value FROM foreign_data", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "retained"
        );
    });
}

#[test]
fn initialized_database_cannot_be_created_again_under_another_identity() {
    with_server(|server, conn| {
        drop(
            Engine::create_remote(
                endpoint(&server),
                &identity(),
                DataOrigin::Synthetic,
                &options(),
            )
            .unwrap(),
        );
        let before = schema(&conn);
        rejected(Engine::create_remote(
            endpoint(&server),
            &RemoteIdentity::new("tenant-beta", "crm").unwrap(),
            DataOrigin::Real,
            &options(),
        ));
        assert_eq!(schema(&conn), before);
        assert_eq!(meta(&conn, "database_id"), "tenant-alpha");
        assert_eq!(meta(&conn, "data_origin"), "synthetic");
    });
}

#[test]
fn opening_empty_or_foreign_database_does_not_create_or_repair_it() {
    with_server(|server, conn| {
        rejected(Engine::open_remote(
            endpoint(&server),
            &identity(),
            &options(),
        ));
        assert_eq!(schema(&conn), Vec::<SchemaEntry>::new());
        conn.execute_batch("CREATE TABLE foreign_data(value TEXT)")
            .unwrap();
        let before = schema(&conn);
        rejected(Engine::open_remote(
            endpoint(&server),
            &identity(),
            &options(),
        ));
        assert_eq!(schema(&conn), before);
    });
}

#[test]
fn every_required_metadata_row_must_exist_and_open_never_recreates_it() {
    with_server(|server, conn| {
        drop(
            Engine::create_remote(
                endpoint(&server),
                &identity(),
                DataOrigin::Synthetic,
                &options(),
            )
            .unwrap(),
        );
        for key in [
            "remote_format",
            "schema_version",
            "database_id",
            "database_kind",
            "data_origin",
            "created_at",
            "crm_schema_version",
            "custom_fields_schema_version",
            "public_data_schema_version",
        ] {
            let saved = meta(&conn, key);
            conn.execute("DELETE FROM meta WHERE key=?1", [key])
                .unwrap();
            rejected(Engine::open_remote(
                endpoint(&server),
                &identity(),
                &options(),
            ));
            let present: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)",
                    [key],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!present);
            conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2)", (key, saved))
                .unwrap();
        }
    });
}

#[test]
fn old_future_and_malformed_schema_or_extension_versions_refuse_open() {
    with_server(|server, conn| {
        drop(
            Engine::create_remote(
                endpoint(&server),
                &identity(),
                DataOrigin::Synthetic,
                &options(),
            )
            .unwrap(),
        );
        for key in [
            "schema_version",
            "crm_schema_version",
            "custom_fields_schema_version",
            "public_data_schema_version",
        ] {
            for value in ["0", "2", "01", "+1", " 1", "one", "4294967296"] {
                conn.execute("UPDATE meta SET value=?2 WHERE key=?1", (key, value))
                    .unwrap();
                let error = rejected(Engine::open_remote(
                    endpoint(&server),
                    &identity(),
                    &options(),
                ));
                if value == "2" {
                    assert!(matches!(
                        error,
                        EngineError::UnsupportedVersion {
                            found: 2,
                            supported: 1
                        }
                    ));
                }
                assert_eq!(meta(&conn, key), value);
            }
            conn.execute("UPDATE meta SET value='1' WHERE key=?1", [key])
                .unwrap();
        }
    });
}

#[test]
fn wrong_identity_kind_format_origin_or_creation_marker_refuse_without_repair() {
    with_server(|server, conn| {
        drop(
            Engine::create_remote(
                endpoint(&server),
                &identity(),
                DataOrigin::Synthetic,
                &options(),
            )
            .unwrap(),
        );
        rejected(Engine::open_remote(
            endpoint(&server),
            &RemoteIdentity::new("tenant-beta", "crm").unwrap(),
            &options(),
        ));
        for (key, value) in [
            ("remote_format", "meibo-remote-v2"),
            ("database_id", "tenant-beta"),
            ("database_kind", "control"),
            ("data_origin", "unknown"),
            ("created_at", "not-a-time"),
        ] {
            let saved = meta(&conn, key);
            conn.execute("UPDATE meta SET value=?2 WHERE key=?1", (key, value))
                .unwrap();
            rejected(Engine::open_remote(
                endpoint(&server),
                &identity(),
                &options(),
            ));
            assert_eq!(meta(&conn, key), value);
            conn.execute("UPDATE meta SET value=?2 WHERE key=?1", (key, saved))
                .unwrap();
        }
    });
}

#[test]
fn partial_schema_is_refused_without_recreating_missing_tables() {
    with_server(|server, conn| {
        drop(
            Engine::create_remote(
                endpoint(&server),
                &identity(),
                DataOrigin::Synthetic,
                &options(),
            )
            .unwrap(),
        );
        conn.execute_batch("DROP TABLE crm_work_items").unwrap();
        let before = schema(&conn);
        rejected(Engine::open_remote(
            endpoint(&server),
            &identity(),
            &options(),
        ));
        assert_eq!(schema(&conn), before);
    });
}

#[test]
fn invalid_identity_options_and_file_endpoint_cannot_create_a_local_database() {
    assert!(RemoteIdentity::new("", "crm").is_err());
    assert!(RemoteIdentity::new("a".repeat(129), "crm").is_err());
    assert!(RemoteIdentity::new("https://private.invalid/secret", "crm").is_err());
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("must-not-exist.db");
    let secret = "synthetic-private-token";
    for url in [
        file.to_str().unwrap().to_owned(),
        format!("file://{}", file.display()),
    ] {
        let error = rejected(Engine::create_remote(
            RemoteOptions::new(&url, secret),
            &identity(),
            DataOrigin::Synthetic,
            &options(),
        ));
        let diagnostic = format!("{error} {error:?}");
        assert!(!diagnostic.contains(&url));
        assert!(!diagnostic.contains(secret));
        assert!(!file.exists());
    }
    with_server(|server, conn| {
        rejected(Engine::create_remote(
            RemoteOptions::new(server.endpoint(), "synthetic-core-token"),
            &identity(),
            DataOrigin::Synthetic,
            &options(),
        ));
        assert_eq!(schema(&conn), Vec::<SchemaEntry>::new());
        let invalid = OpenOptions {
            wal_limit_mib: 0,
            ..options()
        };
        rejected(Engine::create_remote(
            endpoint(&server),
            &identity(),
            DataOrigin::Synthetic,
            &invalid,
        ));
        assert_eq!(schema(&conn), Vec::<SchemaEntry>::new());
        rejected(Engine::create_remote(
            endpoint(&server),
            &RemoteIdentity::new("control-id", "control").unwrap(),
            DataOrigin::Synthetic,
            &options(),
        ));
        assert_eq!(schema(&conn), Vec::<SchemaEntry>::new());
    });
}

#[test]
fn authentication_failure_is_an_actual_remote_refusal_with_private_diagnostics() {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = DeniedServer::start();
    let endpoint = format!("http://{}", server.address);
    let secret = "synthetic-private-token";
    let error = rejected(Engine::create_remote(
        RemoteOptions::new(&endpoint, secret).allow_loopback_http_for_tests(),
        &identity(),
        DataOrigin::Synthetic,
        &options(),
    ));
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    let diagnostic = format!("{error} {error:?}");
    for private in [&endpoint, secret, "SELECT private", "denial-body"] {
        assert!(!diagnostic.contains(private));
    }
}

#[test]
fn connection_timeout_refuses_without_local_fallback() {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}");
    let transport = RemoteOptions::new(&endpoint, "synthetic-timeout-token")
        .allow_loopback_http_for_tests()
        .with_timeout(Duration::from_millis(50))
        .unwrap();
    let error = rejected(Engine::open_remote(transport, &identity(), &options()));
    let diagnostic = format!("{error} {error:?}");
    assert!(!diagnostic.contains(&endpoint));
    assert!(!diagnostic.contains("synthetic-timeout-token"));
    drop(listener);
}

#[test]
fn live_remote_connection_poison_is_observable_after_server_loss() {
    with_server(|server, _conn| {
        let engine = Engine::create_remote(
            endpoint(&server),
            &identity(),
            DataOrigin::Synthetic,
            &options(),
        )
        .unwrap();
        assert!(!engine.is_poisoned());
        drop(server);
        assert!(engine.count(&Filters::default()).is_err());
        assert!(engine.is_poisoned());
    });
}

#[test]
fn cloud_local_engine_keeps_its_file_header_and_nonpoisoned_connection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("local.db");
    let opt = OpenOptions {
        background_checkpoint: false,
        ..OpenOptions::default()
    };
    let engine = Engine::create(&path, DataOrigin::Synthetic, &opt).unwrap();
    assert_eq!(engine.path(), Some(path.as_path()));
    assert!(!engine.is_poisoned());
    let native = rusqlite::Connection::open(&path).unwrap();
    let header: i64 = native
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(header, 0x6d65_6962);
}

struct DeniedServer {
    address: SocketAddr,
    count: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DeniedServer {
    fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let requests = Arc::clone(&count);
        let stop = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut input = Vec::new();
                let mut chunk = [0; 4096];
                while input.len() < 64 * 1024 {
                    let Ok(n) = stream.read(&mut chunk) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    input.extend_from_slice(&chunk[..n]);
                    if let Some(end) = input.windows(4).position(|window| window == b"\r\n\r\n") {
                        let end = end + 4;
                        let headers = String::from_utf8_lossy(&input[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if input.len() >= end.saturating_add(length) {
                            break;
                        }
                    }
                }
                requests.fetch_add(1, Ordering::SeqCst);
                let body = "denial-body SELECT private synthetic-private-token";
                let reply = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        Self {
            address,
            count,
            stopped,
            worker: Some(worker),
        }
    }
}

impl Drop for DeniedServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
