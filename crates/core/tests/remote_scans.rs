//! Bounded production readers against an explicitly enabled real SQL server.
#![cfg(feature = "integration-tests")]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use meibo_core::{
    Engine, OpenOptions, RemoteIdentity, SpaceProbe,
    remote_snapshot::{MAX_ROW_BYTES, SnapshotError, crm_tables, decode_row},
    types::{DataOrigin, EngineError, Field, Filters, Status},
};
use meibo_db::{Connection, RemoteErrorKind, RemoteOptions, Value, remote_error_kind};

#[allow(dead_code)]
#[path = "../../db/tests/support/client.rs"]
mod client;
#[allow(dead_code)]
#[path = "../../db/tests/support/proxy.rs"]
mod proxy;
#[allow(dead_code)]
#[path = "../../db/tests/support/sqld.rs"]
mod sqld;

static FIXTURES: Mutex<()> = Mutex::new(());

struct NoFilesystem;
impl SpaceProbe for NoFilesystem {
    fn available(&self, _: &std::path::Path) -> std::io::Result<u64> {
        panic!("remote readers and maintenance must not inspect local capacity");
    }
}

fn endpoint(server: &sqld::Sqld) -> RemoteOptions {
    RemoteOptions::new(server.endpoint(), "synthetic-scan-token").allow_loopback_http_for_tests()
}

fn with_database(run: impl FnOnce(Engine, Connection, &sqld::Sqld)) {
    let _guard = FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = sqld::Sqld::start().unwrap();
    let engine = Engine::create_remote(
        endpoint(&server),
        &RemoteIdentity::new("scan-alpha", "crm").unwrap(),
        DataOrigin::Synthetic,
        &OpenOptions {
            space_probe: Some(Arc::new(NoFilesystem)),
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let conn = Connection::open_remote(endpoint(&server)).unwrap();
    run(engine, conn, &server);
}

fn seed(conn: &Connection, n: u32) {
    conn.execute_batch(&format!(
        "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<{n})
         INSERT INTO companies(id,name,name_key,prefecture_code,status,base_status,created_at,updated_at)
         SELECT i,printf('synthetic-%04d',i),printf('synthetic-%04d',i),13+i%2,i%6,i%6,1,1 FROM n WHERE i<={n};
         INSERT INTO name_fts(rowid,name_key) SELECT id,name_key FROM companies;
         UPDATE counts SET n=(SELECT count(*) FROM companies c WHERE c.status=counts.status AND c.has_phone=counts.has_phone);"
    )).unwrap();
}

fn bounded_engine(server: &sqld::Sqld, bytes: usize) -> Engine {
    Engine::open_remote(
        endpoint(server).with_response_limit(bytes).unwrap(),
        &RemoteIdentity::new("scan-alpha", "crm").unwrap(),
        &OpenOptions::default(),
    )
    .unwrap()
}

#[test]
fn csv_reader_crosses_all_boundaries_without_an_unbounded_response() {
    for count in [0, 1, 499, 500, 501, 1201] {
        with_database(|_, conn, server| {
            seed(&conn, count);
            let engine = bounded_engine(server, 512 * 1024);
            let mut ids = Vec::new();
            let n = engine
                .visit_companies(
                    &Filters {
                        include_excluded: true,
                        ..Filters::default()
                    },
                    &vec![Field::Name],
                    |company| {
                        ids.push(company.id.0);
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(n, u64::from(count));
            assert_eq!(ids, (1..=i64::from(count)).collect::<Vec<_>>());
        });
    }
}

#[test]
fn csv_preserves_filter_order_erasure_and_one_read_snapshot() {
    with_database(|_, conn, server| {
        seed(&conn, 1201);
        conn.execute("UPDATE companies SET erased_at=1 WHERE id%101=0", [])
            .unwrap();
        let engine = bounded_engine(server, 512 * 1024);
        let mut grouped = Vec::new();
        engine
            .visit_companies(
                &Filters {
                    region: vec![
                        meibo_core::types::Region::Prefecture(13),
                        meibo_core::types::Region::Prefecture(14),
                    ],
                    include_excluded: true,
                    ..Filters::default()
                },
                &vec![Field::Name],
                |company| {
                    grouped.push(company.id.0);
                    Ok(())
                },
            )
            .unwrap();
        let expected: Vec<_> = [0, 1]
            .into_iter()
            .flat_map(|parity| (1..=1201).filter(move |id| id % 2 == parity && id % 101 != 0))
            .collect();
        // The compiled prefecture shape continues within a group and crosses
        // to the next group after a 500-row page, preserving its full order.
        let mut expected = expected;
        expected.sort_by_key(|id| (id % 2, id % 6, *id));
        assert_eq!(grouped, expected);
        assert!(matches!(
            engine.visit_companies(&Filters::default(), &vec![Field::Name], |_| Err(
                EngineError::NotFound
            )),
            Err(EngineError::NotFound)
        ));
        let filters = Filters {
            status: vec![Status::Untouched, Status::Excluded],
            include_excluded: true,
            ..Filters::default()
        };
        let mut first = true;
        let mut ids = Vec::new();
        engine.visit_companies(&filters, &vec![Field::Name], |company| {
            if first {
                first = false;
                conn.execute("INSERT INTO companies(id,name,name_key,prefecture_code,status,base_status,created_at,updated_at) VALUES(5000,'late','late',13,5,5,1,1)", []).unwrap();
            }
            ids.push(company.id.0);
            Ok(())
        }).unwrap();
        let expected: Vec<_> = [0, 5]
            .into_iter()
            .flat_map(|status| (1..=1201).filter(move |id| id % 6 == status && id % 101 != 0))
            .collect();
        assert_eq!(ids, expected);
        assert!(!engine.is_poisoned());
    });
}

#[test]
fn fold_verification_keeps_one_company_state_across_many_pages_and_undo() {
    with_database(|_, conn, server| {
        seed(&conn, 2);
        conn.execute_batch(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1201)
             INSERT INTO activities(id,company_id,kind,at,status_after,next_action_kind,next_action_at)
             SELECT i,1,1,i,CASE WHEN i=1001 THEN 4 ELSE 2 END,2,i+100 FROM n;
             INSERT INTO activities(id,company_id,kind,at,next_action_kind,reverses_id) VALUES(1202,1,9,1202,0,1201);
             INSERT INTO activities(id,company_id,kind,at,next_action_kind) VALUES(1203,2,4,80,0);
             UPDATE companies SET status=2,last_activity_at=1200,next_action_at=1300 WHERE id=1;
             UPDATE companies SET last_activity_at=80 WHERE id=2;
             UPDATE counts SET n=(SELECT count(*) FROM companies c WHERE c.status=counts.status AND c.has_phone=counts.has_phone);"
        ).unwrap();
        let mut engine = bounded_engine(server, 240 * 1024);
        let report = engine.verify().unwrap();
        assert!(
            report.integrity_ok && report.counts_match && report.folds_match,
            "{report:?}"
        );
        engine
            .record_activity(&meibo_core::types::ActivityInput {
                company: meibo_core::types::CompanyId(1),
                contact: None,
                kind: meibo_core::types::ActivityKind::Cancel,
                at: 9999,
                note: None,
                status_after: None,
                next_action: meibo_core::types::NextAction::Keep,
                reverses: Some(meibo_core::types::ActivityId(1200)),
            })
            .unwrap();
        assert!(engine.verify().unwrap().folds_match);
        assert_eq!(
            conn.query_row(
                "SELECT last_activity_at,next_action_at FROM companies WHERE id=1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
            (1199, 1299)
        );
        conn.execute("UPDATE companies SET next_action_at=1301 WHERE id=1", [])
            .unwrap();
        assert!(!engine.verify().unwrap().folds_match);
    });
}

#[test]
fn snapshot_visits_integer_composite_text_and_blob_keys_with_exact_rows() {
    with_database(|engine, conn, _| {
        seed(&conn, 1201);
        conn.execute_batch(
            "INSERT INTO lists(id,name,created_at) VALUES(1,'one',1),(2,'two',1);
             INSERT INTO list_members SELECT 1,id FROM companies;
             INSERT INTO list_members SELECT 2,id FROM companies WHERE id%2=0;
             WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1101)
             INSERT INTO meta(key,value) SELECT printf('snapshot-%04d',i),printf('value-%d',i) FROM n;
             WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1101)
             INSERT INTO erased_corporate_numbers(number_hash) SELECT CAST(printf('%032d',i) AS BLOB) FROM n;
             INSERT INTO erased_corporate_numbers(number_hash) VALUES(X'00FF'),(X'FF00'),(X'0000');
             INSERT INTO custom_field_definitions(id,name,kind,is_indexed) VALUES(1,'a',1,0),(9000,'deleted',1,0);
             DELETE FROM custom_field_definitions WHERE id=9000;
             INSERT INTO custom_field_values(field_id,company_id,kind,is_indexed,text_value) VALUES(1,1,1,0,'typed text');
             INSERT INTO contacts(id,company_id,name,created_at) VALUES(1,1,'synthetic contact',1);
             INSERT INTO activities(id,company_id,contact_id,kind,at,next_action_kind) VALUES(1,1,1,4,1,0);
             INSERT INTO crm_work_items(id,company_id,kind,subject,due_at,completed,created_at,updated_at) VALUES(1,1,1,'synthetic task',1,0,1,1);
             INSERT INTO crm_opportunities(id,company_id,name,amount_jpy,stage,close_date,created_at,updated_at) VALUES(1,1,'synthetic opportunity',42,0,'2026-12-01',1,1);"
        ).unwrap();
        let mut got = BTreeMap::<String, Vec<Vec<Value>>>::new();
        let sequences = engine
            .visit_crm_snapshot(|table, bytes| {
                got.entry(table.name().into())
                    .or_default()
                    .push(decode_row(bytes, table.columns().len()).unwrap());
                Ok(())
            })
            .unwrap();
        assert_eq!(sequences.high_water(), 9000);
        for table in crm_tables() {
            let sql = format!(
                "SELECT {} FROM {} ORDER BY {}",
                table.columns().join(","),
                table.name(),
                table.key_columns().join(",")
            );
            let mut statement = conn.prepare(&sql).unwrap();
            let expected = statement
                .query_map([], |row| {
                    (0..table.columns().len())
                        .map(|index| row.get::<_, Value>(index))
                        .collect::<meibo_db::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<meibo_db::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(
                got.get(table.name()).map_or(&[][..], Vec::as_slice),
                expected,
                "{}",
                table.name()
            );
        }
    });
}

#[test]
fn snapshot_does_not_read_later_commits_and_returns_callback_failures() {
    with_database(|engine, conn, _| {
        seed(&conn, 1201);
        let mut ids = Vec::new();
        engine.visit_crm_snapshot(|table, bytes| {
            if table.name()=="companies" {
                let row = decode_row(bytes, table.columns().len()).unwrap();
                if ids.is_empty() {
                    conn.execute("INSERT INTO companies(id,name,name_key,prefecture_code,created_at,updated_at) VALUES(5000,'late','late',13,1,1)", []).unwrap();
                }
                ids.push(row[0].clone());
            }
            Ok(())
        }).unwrap();
        assert_eq!(ids, (1..=1201).map(Value::Integer).collect::<Vec<_>>());
        assert!(matches!(
            engine.visit_crm_snapshot(|_, _| Err(EngineError::NotFound)),
            Err(EngineError::NotFound)
        ));
        assert!(!engine.is_poisoned());
        assert_eq!(
            engine
                .company_detail(meibo_core::types::CompanyId(5000), &vec![Field::Name])
                .unwrap()
                .company
                .id
                .0,
            5000
        );
    });
}

#[test]
fn snapshot_rejects_erase_gate_oversized_rows_and_invalid_sequences() {
    with_database(|engine, conn, _| {
        conn.execute("INSERT INTO erase_gate VALUES(1)", [])
            .unwrap();
        let mut calls = 0;
        assert!(matches!(
            engine.visit_crm_snapshot(|_, _| {
                calls += 1;
                Ok(())
            }),
            Err(EngineError::Corrupt)
        ));
        assert_eq!(calls, 0);
        conn.execute("DELETE FROM erase_gate", []).unwrap();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('too-large',?1)",
            ["x".repeat(MAX_ROW_BYTES)],
        )
        .unwrap();
        snapshot_error(
            engine.visit_crm_snapshot(|_, _| Ok(())).unwrap_err(),
            SnapshotError::LimitExceeded,
        );
        conn.execute("DELETE FROM meta WHERE key='too-large'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO sqlite_sequence(name,seq) VALUES('unknown',1)",
            [],
        )
        .unwrap();
        snapshot_error(
            engine.visit_crm_snapshot(|_, _| Ok(())).unwrap_err(),
            SnapshotError::InvalidSequence,
        );
        conn.execute("DELETE FROM sqlite_sequence WHERE name='unknown'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO erased_corporate_numbers(number_hash) VALUES(NULL)",
            [],
        )
        .unwrap();
        assert!(matches!(
            engine.visit_crm_snapshot(|_, _| Ok(())),
            Err(EngineError::Corrupt)
        ));
        conn.execute(
            "DELETE FROM erased_corporate_numbers WHERE number_hash IS NULL",
            [],
        )
        .unwrap();
        engine.visit_crm_snapshot(|_, _| Ok(())).unwrap();
        assert!(!engine.is_poisoned());
    });
}

#[test]
fn snapshot_pages_wide_valid_rows_below_transport_and_workspace_caps() {
    with_database(|engine, conn, _| {
        conn.execute_batch(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<25)
             INSERT INTO meta(key,value) SELECT printf('wide-%04d',i),replace(hex(zeroblob(450000)),'0','x') FROM n;"
        ).unwrap();
        let mut wide = 0;
        engine
            .visit_crm_snapshot(|table, bytes| {
                assert!(bytes.len() <= MAX_ROW_BYTES);
                if table.name() == "meta" && bytes.len() > 900_000 {
                    wide += 1;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(wide, 25);
    });
}

fn snapshot_error(error: EngineError, expected: SnapshotError) {
    let EngineError::Storage(error) = error else {
        panic!("expected fixed snapshot error");
    };
    assert_eq!(error.downcast_ref::<SnapshotError>(), Some(&expected));
}

fn unsupported(error: EngineError) {
    let EngineError::Storage(error) = error else {
        panic!("expected fixed unsupported storage error");
    };
    assert_eq!(
        remote_error_kind(error.downcast_ref::<meibo_db::Error>().unwrap()),
        Some(RemoteErrorKind::Unsupported)
    );
}

#[test]
fn remote_file_and_wal_maintenance_is_explicitly_unsupported_without_local_files() {
    with_database(|mut engine, conn, server| {
        let dest = server.directory().join("must-not-exist.db");
        unsupported(engine.compact().unwrap_err());
        unsupported(engine.settle().unwrap_err());
        unsupported(engine.backup_to(&dest).unwrap_err());
        assert!(!dest.exists());
        assert!(!engine.is_poisoned());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM companies", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    });
}

#[test]
fn completed_scans_report_lost_snapshot_close_and_do_not_reuse_the_connection() {
    with_database(|_, conn, server| {
        seed(&conn, 1);
        let proxy = proxy::FaultProxy::start(server.address()).unwrap();
        for operation in 0..3 {
            let engine = Engine::open_remote(
                RemoteOptions::new(proxy.endpoint(), "synthetic-scan-token")
                    .allow_loopback_http_for_tests(),
                &RemoteIdentity::new("scan-alpha", "crm").unwrap(),
                &OpenOptions::default(),
            )
            .unwrap();
            proxy.arm(proxy::Fault::LoseRollbackResponse);
            let error = match operation {
                0 => engine.visit_crm_snapshot(|_, _| Ok(())).unwrap_err(),
                1 => engine
                    .visit_companies(&Filters::default(), &vec![Field::Name], |_| Ok(()))
                    .unwrap_err(),
                _ => engine.verify().unwrap_err(),
            };
            let EngineError::Storage(error) = error else {
                panic!("expected fixed storage outcome");
            };
            assert_eq!(
                remote_error_kind(error.downcast_ref::<meibo_db::Error>().unwrap()),
                Some(RemoteErrorKind::OutcomeUnknown)
            );
            assert!(engine.is_poisoned());
            let requests = proxy.requests();
            assert!(engine.count(&Filters::default()).is_err());
            assert_eq!(proxy.requests(), requests);
        }
        for operation in 0..2 {
            let engine = Engine::open_remote(
                RemoteOptions::new(proxy.endpoint(), "synthetic-scan-token")
                    .allow_loopback_http_for_tests(),
                &RemoteIdentity::new("scan-alpha", "crm").unwrap(),
                &OpenOptions::default(),
            )
            .unwrap();
            proxy.arm(proxy::Fault::LoseRollbackResponse);
            let error = if operation == 0 {
                engine
                    .visit_crm_snapshot(|_, _| Err(EngineError::NotFound))
                    .unwrap_err()
            } else {
                engine
                    .visit_companies(&Filters::default(), &vec![Field::Name], |_| {
                        Err(EngineError::NotFound)
                    })
                    .unwrap_err()
            };
            assert!(matches!(error, EngineError::NotFound));
            assert!(engine.is_poisoned());
        }
        assert_eq!(proxy.triggered(), 5);
    });
}
