//! Production transport against real sqld and adversarial loopback responses.
#[path = "support/response.rs"]
mod response;
#[path = "support/sqld.rs"]
mod sqld;

use meibo_db::{
    Error, ErrorCode, OptionalExtension, RemoteConnection, RemoteOptions, params, params_from_iter,
};
use response::{ResponseServer, response};
use std::time::{Duration, Instant};

// The external sqld fixture releases an ephemeral port before the child binds
// it. Keep this binary's raw listeners and child launches from competing for
// that reservation. Production runtime concurrency is tested independently.
static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

macro_rules! remote_test {
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
    RemoteOptions::new(endpoint, "synthetic-bearer-secret").allow_loopback_http_for_tests()
}

fn connect(endpoint: &str) -> RemoteConnection {
    RemoteConnection::connect(options(endpoint)).unwrap()
}

fn no_secrets(error: &Error, endpoint: &str) {
    let output = format!("{error} {error:?}");
    for secret in [
        endpoint,
        "synthetic-bearer-secret",
        "synthetic_sql_secret",
        "raw-body-secret",
    ] {
        assert!(
            !output.contains(secret),
            "diagnostic contains synthetic secret"
        );
    }
}

fn setup_body() -> serde_json::Value {
    serde_json::json!({
        "baton": "synthetic-stream", "base_url": null,
        "results": [
            {"type":"ok","response":{"type":"store_sql"}},
            {"type":"ok","response":{"type":"execute","result":{
                "cols":[],"rows":[],"affected_row_count":0,"last_insert_rowid":null
            }}},
            {"type":"ok","response":{"type":"execute","result":{
                "cols":[{"name":"foreign_keys","decltype":null}],
                "rows":[[{"type":"integer","value":"1"}]],
                "affected_row_count":0,"last_insert_rowid":null
            }}}
        ]
    })
}

fn json_response(body: &serde_json::Value) -> Vec<u8> {
    response(200, &serde_json::to_vec(body).unwrap(), false, "")
}

remote_test!(options_enforce_deadline_and_response_limits, {
    let opts = RemoteOptions::new("https://synthetic.invalid", "secret");
    assert_eq!(opts.timeout(), Duration::from_secs(30));
    assert_eq!(opts.response_limit(), 16 * 1024 * 1024);
    for timeout in [Duration::ZERO, Duration::from_secs(121)] {
        assert!(opts.clone().with_timeout(timeout).is_err());
    }
    for limit in [0, 32 * 1024 * 1024 + 1] {
        assert!(opts.clone().with_response_limit(limit).is_err());
    }
    assert!(opts.clone().with_timeout(Duration::from_secs(120)).is_ok());
    assert!(opts.with_response_limit(32 * 1024 * 1024).is_ok());
});

remote_test!(
    production_schemes_credentials_and_loopback_are_restricted,
    {
        for endpoint in [
            "http://127.0.0.1:8080",
            "http://localhost:8080",
            "http://192.0.2.1",
            "ftp://synthetic.invalid",
            "libsql+ws://synthetic.invalid",
            "file:///tmp/db",
            "https://user:pass@synthetic.invalid",
            "https://@synthetic.invalid",
            "https:synthetic.invalid",
            "libsql:synthetic.invalid",
            "https:///synthetic.invalid",
            " https://synthetic.invalid",
            "https://synthetic.\ninvalid",
            "https://synthetic.invalid/?key=secret",
            "https://synthetic.invalid/#secret",
            "https://synthetic.invalid/path",
            "not a URL",
        ] {
            let opts = RemoteOptions::new(endpoint, "synthetic-bearer-secret");
            let debug = format!("{opts:?}");
            assert!(!debug.contains(endpoint));
            assert!(!debug.contains("synthetic-bearer-secret"));
            let error = RemoteConnection::connect(opts).unwrap_err();
            no_secrets(&error, endpoint);
        }
        for endpoint in [
            "http://localhost:8080",
            "http://192.0.2.1",
            "http://127.0.0.1:8080/path",
        ] {
            assert!(RemoteConnection::connect(options(endpoint)).is_err());
        }
        for endpoint in ["https://synthetic.invalid", "libsql://synthetic.invalid"] {
            let connection =
                RemoteConnection::connect(RemoteOptions::new(endpoint, "secret")).unwrap();
            assert!(!format!("{connection:?}").contains(endpoint));
        }
    }
);

remote_test!(real_sqld_keeps_exact_typed_parameters_and_owned_rows, {
    let server = sqld::Sqld::start().unwrap();
    assert!(server.directory().is_dir());
    assert!(server.address().ip().is_loopback());
    let connection = connect(&server.endpoint());
    connection.execute_batch("CREATE TABLE synthetic(id INTEGER PRIMARY KEY, text TEXT, blob BLOB, optional INTEGER, real REAL)").unwrap();
    for id in [i64::MIN, i64::MAX] {
        assert_eq!(
            connection
                .execute(
                    "INSERT INTO synthetic VALUES(?1,?2,?3,?4,?5)",
                    params![id, "会社\0🦀", vec![0_u8, 255, 7], None::<i64>, 1.25_f64]
                )
                .unwrap(),
            1
        );
        assert_eq!(connection.last_insert_rowid(), id);
    }
    let mut statement = connection
        .prepare_cached("SELECT * FROM synthetic WHERE id=?1")
        .unwrap();
    assert_eq!(statement.column_count(), 5);
    for id in [i64::MAX, i64::MIN, i64::MAX] {
        let mut rows = statement.query(params_from_iter([id])).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(row.get::<_, i64>("ID").unwrap(), id);
        assert_eq!(row.get::<_, String>(1).unwrap(), "会社\0🦀");
        assert_eq!(row.get::<_, Vec<u8>>(2).unwrap(), [0, 255, 7]);
        assert_eq!(row.get::<_, Option<i64>>(3).unwrap(), None);
        assert_eq!(row.get::<_, f64>(4).unwrap(), 1.25);
        assert!(rows.next().unwrap().is_none());
    }
    assert!(matches!(
        statement.query([1, 2]),
        Err(Error::InvalidParameterCount(2, 1))
    ));
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            connection
                .execute("INSERT INTO synthetic(id) VALUES(?1)", [value])
                .is_err()
        );
    }
});

remote_test!(
    real_sqld_query_map_exists_missing_rows_and_conversion_match_local,
    {
        let server = sqld::Sqld::start().unwrap();
        let connection = connect(&server.endpoint());
        let mut statement = connection
            .prepare("SELECT ?1 AS value UNION ALL SELECT ?1 + 1")
            .unwrap();
        let values = statement
            .query_map([41], |row| row.get::<_, i64>("value"))
            .unwrap()
            .collect::<meibo_db::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(values, [41, 42]);
        assert!(statement.exists([41]).unwrap());
        assert!(
            !connection
                .prepare("SELECT 1 WHERE 0")
                .unwrap()
                .exists([])
                .unwrap()
        );
        assert_eq!(
            connection
                .query_row("SELECT 1 WHERE 0", [], |row| row.get::<_, i64>(0))
                .optional()
                .unwrap(),
            None
        );
        assert!(matches!(
            connection.query_row("SELECT 'text'", [], |row| row.get::<_, i64>(0)),
            Err(Error::InvalidColumnType(..))
        ));
        assert!(matches!(
            connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(3)),
            Err(Error::InvalidColumnIndex(3))
        ));
        assert!(matches!(
            connection.execute("SELECT 1", []),
            Err(Error::ExecuteReturnedResults)
        ));
    }
);

remote_test!(
    real_sqld_sql_errors_are_compatible_sanitized_and_do_not_poison,
    {
        let server = sqld::Sqld::start().unwrap();
        let connection = connect(&server.endpoint());
        connection
            .execute(
                "CREATE TABLE synthetic(id INTEGER PRIMARY KEY, value TEXT UNIQUE)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO synthetic VALUES(1,'synthetic_sql_secret')", [])
            .unwrap();
        let error = connection
            .execute("INSERT INTO synthetic VALUES(2,'synthetic_sql_secret')", [])
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(ErrorCode::ConstraintViolation)
        );
        no_secrets(&error, &server.endpoint());
        let error = connection
            .query_row("SELECT 'synthetic_sql_secret'", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_err();
        no_secrets(&error, &server.endpoint());
        assert!(!connection.is_poisoned());
        let error = connection
            .execute("synthetic_sql_secret invalid sql", [])
            .unwrap_err();
        no_secrets(&error, &server.endpoint());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM synthetic", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
);

remote_test!(
    real_sqld_fresh_streams_enforce_foreign_keys_on_every_operation,
    {
        let server = sqld::Sqld::start().unwrap();
        let connection = connect(&server.endpoint());
        connection.execute_batch("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(id INTEGER PRIMARY KEY, parent INTEGER REFERENCES parent(id)); INSERT INTO parent VALUES(7)").unwrap();
        for id in [1, 2, 3] {
            let error = connection
                .execute("INSERT INTO child VALUES(?1,999)", [id])
                .unwrap_err();
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
        }
        connection
            .execute("INSERT INTO child VALUES(1,7)", [])
            .unwrap();
        assert!(
            connection
                .execute("DELETE FROM parent WHERE id=7", [])
                .is_err()
        );
    }
);

remote_test!(real_sqld_sequence_preserves_complete_trigger_sql, {
    let server = sqld::Sqld::start().unwrap();
    let connection = connect(&server.endpoint());
    connection.execute_batch("CREATE TABLE synthetic(value); CREATE TABLE audit(value); CREATE TRIGGER synthetic_ai AFTER INSERT ON synthetic BEGIN INSERT INTO audit VALUES('semi;colon'); INSERT INTO audit VALUES(new.value); END; INSERT INTO synthetic VALUES('inserted')").unwrap();
    let values = connection
        .prepare("SELECT value FROM audit ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<meibo_db::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(values, ["semi;colon", "inserted"]);
});

remote_test!(
    real_sqld_allows_five_hundred_full_rows_in_one_bounded_page,
    {
        let server = sqld::Sqld::start().unwrap();
        let connection = connect(&server.endpoint());
        let mut statement = connection.prepare("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<500) SELECT x, ?1, ?2, NULL, x*1.5 FROM n").unwrap();
        let values = statement
            .query_map(params!["synthetic".repeat(100), vec![7_u8; 1024]], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?.len(),
                    row.get::<_, Vec<u8>>(2)?.len(),
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .unwrap()
            .collect::<meibo_db::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(values.len(), 500);
        assert_eq!(values[499], (500, 900, 1024, None));
    }
);

remote_test!(
    success_and_error_bodies_are_bounded_with_length_and_chunked_encoding,
    {
        for status in [200, 401, 503] {
            for chunked in [false, true] {
                let server = ResponseServer::start(
                    response(status, &vec![b'x'; 64 * 1024], chunked, ""),
                    Duration::ZERO,
                );
                let connection = RemoteConnection::connect(
                    options(&server.endpoint())
                        .with_response_limit(4096)
                        .unwrap(),
                )
                .unwrap();
                let error = connection.execute("synthetic_sql_secret", []).unwrap_err();
                no_secrets(&error, &server.endpoint());
                assert!(error.to_string().contains("response exceeds limit"));
                assert!(connection.is_poisoned());
                assert_eq!(server.count(), 1);
            }
        }
    }
);

remote_test!(deadline_includes_body_and_connection_is_never_retried, {
    for body_only in [false, true] {
        let server = ResponseServer::script(
            vec![json_response(&setup_body())],
            Duration::from_millis(180),
            body_only,
        );
        let connection = RemoteConnection::connect(
            options(&server.endpoint())
                .with_timeout(Duration::from_millis(35))
                .unwrap(),
        )
        .unwrap();
        let start = Instant::now();
        let error = connection.execute("synthetic_sql_secret", []).unwrap_err();
        assert!(start.elapsed() < Duration::from_millis(150));
        no_secrets(&error, &server.endpoint());
        assert!(connection.is_poisoned());
        assert!(connection.execute("SELECT 1", []).is_err());
        assert_eq!(server.count(), 1);
    }
});

remote_test!(redirects_never_forward_authorization_or_retry, {
    let target = ResponseServer::start(response(200, b"{}", false, ""), Duration::ZERO);
    let server = ResponseServer::start(
        response(
            307,
            b"raw-body-secret",
            false,
            &format!("Location: {}/v2/pipeline\r\n", target.endpoint()),
        ),
        Duration::ZERO,
    );
    let connection = connect(&server.endpoint());
    let error = connection.execute("synthetic_sql_secret", []).unwrap_err();
    no_secrets(&error, &server.endpoint());
    assert_eq!(target.count(), 0);
    assert_eq!(server.count(), 1);
});

remote_test!(
    authentication_invalid_json_and_incomplete_responses_never_leak,
    {
        let mut truncated = response(200, b"raw-body-secret", false, "");
        truncated.truncate(truncated.len() - 3);
        for bytes in [
            response(401, b"raw-body-secret", false, ""),
            response(200, b"raw-body-secret", false, ""),
            truncated,
        ] {
            let server = ResponseServer::start(bytes, Duration::ZERO);
            let connection = connect(&server.endpoint());
            let error = connection.execute("synthetic_sql_secret", []).unwrap_err();
            no_secrets(&error, &server.endpoint());
            assert!(connection.is_poisoned());
            assert!(connection.prepare("SELECT 1").is_err());
            assert_eq!(server.count(), 1);
        }
    }
);

remote_test!(
    pipeline_response_count_variant_baton_and_base_url_are_checked,
    {
        let mut wrong_count = setup_body();
        wrong_count["results"].as_array_mut().unwrap().pop();
        let mut wrong_variant = setup_body();
        wrong_variant["results"][0]["response"] = serde_json::json!({"type":"sequence"});
        let mut missing_baton = setup_body();
        missing_baton["baton"] = serde_json::Value::Null;
        let mut empty_baton = setup_body();
        empty_baton["baton"] = "".into();
        let mut missing_variant = setup_body();
        missing_variant["results"][0] = serde_json::json!({"type":"none"});
        for body in [
            wrong_count,
            wrong_variant,
            missing_baton,
            empty_baton,
            missing_variant,
        ] {
            let server = ResponseServer::start(json_response(&body), Duration::ZERO);
            let connection = connect(&server.endpoint());
            let error = connection.execute("synthetic_sql_secret", []).unwrap_err();
            no_secrets(&error, &server.endpoint());
            assert!(connection.is_poisoned());
            assert_eq!(server.count(), 1);
        }
    }
);

remote_test!(predecode_rejects_row_and_workspace_amplification, {
    // Tiny JSON rows and many nested containers can otherwise allocate much
    // more memory than the body cap during official typed deserialization.
    for body in [
        format!(r#"{{"rows":[{}]}}"#, vec!["[]"; 10_001].join(",")),
        format!(r#"{{"amplification":[{}]}}"#, vec!["[]"; 600_000].join(",")),
    ] {
        let server =
            ResponseServer::start(response(200, body.as_bytes(), true, ""), Duration::ZERO);
        let connection = connect(&server.endpoint());
        let error = connection.execute("SELECT 1", []).unwrap_err();
        assert!(error.to_string().contains("excessive"));
        assert!(connection.is_poisoned());
        assert_eq!(server.count(), 1);
    }
});

remote_test!(
    server_base_url_rejects_cross_origin_credentials_query_and_fragment,
    {
        let target = ResponseServer::start(response(200, b"{}", false, ""), Duration::ZERO);
        for base in [
            target.endpoint(),
            "https://foreign.invalid/".into(),
            "http://user:pass@127.0.0.1/".into(),
            "http://@127.0.0.1/".into(),
            "http://127.0.0.1/?secret".into(),
            "http://127.0.0.1/#secret".into(),
        ] {
            let mut body = setup_body();
            body["base_url"] = base.into();
            let server = ResponseServer::start(json_response(&body), Duration::ZERO);
            let connection = connect(&server.endpoint());
            let error = connection.prepare("synthetic_sql_secret").err().unwrap();
            no_secrets(&error, &server.endpoint());
            assert!(connection.is_poisoned());
            assert_eq!(server.count(), 1);
            assert_eq!(target.count(), 0);
        }
    }
);

remote_test!(setup_failure_closes_once_without_sending_any_user_sql, {
    for failed_step in 0..4 {
        let mut body = setup_body();
        if failed_step < 3 {
            body["results"][failed_step] = serde_json::json!({"type":"error","error":{"code":"SQLITE_ERROR","message":"raw-body-secret"}});
        } else {
            body["results"][2]["response"]["result"]["rows"][0][0]["value"] = "0".into();
        }
        let closed = serde_json::json!({"baton":null,"base_url":null,"results":[{"type":"ok","response":{"type":"close"}}]});
        let server = ResponseServer::script(
            vec![json_response(&body), json_response(&closed)],
            Duration::ZERO,
            false,
        );
        let connection = connect(&server.endpoint());
        let error = connection.execute("synthetic_sql_secret", []).unwrap_err();
        no_secrets(&error, &server.endpoint());
        assert!(connection.is_poisoned());
        assert_eq!(
            server.request_types(),
            [vec!["store_sql", "execute", "execute"], vec!["close"]]
        );
        assert!(connection.execute_batch("SELECT 1").is_err());
        assert_eq!(server.count(), 2);
    }
});

remote_test!(real_sqld_rejects_excess_rows_before_typed_collection, {
    let server = sqld::Sqld::start().unwrap();
    let connection = connect(&server.endpoint());
    let mut statement = connection.prepare("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10001) SELECT x FROM n").unwrap();
    let error = statement.query([]).err().unwrap();
    assert!(error.to_string().contains("excessive"));
    assert!(connection.is_poisoned());
});

remote_test!(real_sqld_remote_calls_work_from_an_async_host_runtime, {
    let server = sqld::Sqld::start().unwrap();
    let connection = connect(&server.endpoint());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert_eq!(
            connection
                .query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            42
        );
    });
});

remote_test!(
    real_sqld_requests_from_four_sync_threads_share_the_runtime,
    {
        let server = sqld::Sqld::start().unwrap();
        let endpoint = server.endpoint();
        let expected_runtime = meibo_db::runtime().unwrap();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0_i64..4)
                .map(|value| {
                    let endpoint = &endpoint;
                    scope.spawn(move || {
                        assert!(std::ptr::eq(expected_runtime, meibo_db::runtime().unwrap()));
                        connect(endpoint)
                            .query_row("SELECT ?1", [value], |row| row.get::<_, i64>(0))
                            .unwrap()
                    })
                })
                .collect();
            let values: Vec<_> = handles
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect();
            assert_eq!(values, [0, 1, 2, 3]);
        });
    }
);

remote_test!(exact_response_body_limit_succeeds_and_one_less_rejects, {
    let setup = serde_json::to_vec(&setup_body()).unwrap();
    let described = serde_json::json!({"baton":null,"base_url":null,"results":[
        {"type":"ok","response":{"type":"describe","result":{"params":[],"cols":[],"is_explain":false,"is_readonly":true}}},
        {"type":"ok","response":{"type":"close"}}
    ]});
    assert!(serde_json::to_vec(&described).unwrap().len() < setup.len());
    for (limit, accepted) in [(setup.len(), true), (setup.len() - 1, false)] {
        let server = ResponseServer::script(
            vec![response(200, &setup, true, ""), json_response(&described)],
            Duration::ZERO,
            false,
        );
        let connection = RemoteConnection::connect(
            options(&server.endpoint())
                .with_response_limit(limit)
                .unwrap(),
        )
        .unwrap();
        let result = connection.prepare("synthetic_sql_secret");
        assert_eq!(result.is_ok(), accepted);
        if !accepted {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("response exceeds limit")
            );
        }
    }
});

remote_test!(close_baton_variant_and_failure_are_validated_after_setup, {
    let good = serde_json::json!({"baton":null,"base_url":null,"results":[
        {"type":"ok","response":{"type":"sequence"}},
        {"type":"ok","response":{"type":"close"}}
    ]});
    let mut open = good.clone();
    open["baton"] = "still-open".into();
    let mut wrong_variant = good.clone();
    wrong_variant["results"][1]["response"] = serde_json::json!({"type":"sequence"});
    let mut close_failed = good;
    close_failed["results"][1] = serde_json::json!({"type":"error","error":{"code":"SQLITE_ERROR","message":"raw-body-secret"}});
    for body in [open, wrong_variant, close_failed] {
        let server = ResponseServer::script(
            vec![json_response(&setup_body()), json_response(&body)],
            Duration::ZERO,
            false,
        );
        let connection = connect(&server.endpoint());
        let error = connection
            .execute_batch("synthetic_sql_secret")
            .unwrap_err();
        no_secrets(&error, &server.endpoint());
        assert!(connection.is_poisoned());
        assert_eq!(server.count(), 2);
    }
});

remote_test!(
    successful_http1_keepalive_responses_still_use_fresh_tcp_connections,
    {
        let described = serde_json::json!({"baton":null,"base_url":null,"results":[
            {"type":"ok","response":{"type":"describe","result":{"params":[],"cols":[],"is_explain":false,"is_readonly":true}}},
            {"type":"ok","response":{"type":"close"}}
        ]});
        let server = ResponseServer::keepalive(vec![
            json_response(&setup_body()),
            json_response(&described),
        ]);
        let connection = connect(&server.endpoint());
        connection.prepare("SELECT 1").unwrap();
        assert_eq!(
            server.request_types(),
            [
                vec!["store_sql", "execute", "execute"],
                vec!["describe", "close"]
            ]
        );
        assert_eq!(
            server.count(),
            2,
            "idle HTTP/1 sockets must not be reused for internal retries"
        );
    }
);
