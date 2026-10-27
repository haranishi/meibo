//! Behavioral compatibility of the local adapter with actual SQLite.

use std::{cell::Cell, time::Duration};

use meibo_db::{
    Connection, Error, ErrorCode, MAIN_DB, OpenFlags, OptionalExtension, Result, Row,
    TransactionBehavior, Value, params, params_from_iter,
};

fn memory() -> Connection {
    Connection::open(":memory:").unwrap()
}

fn count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT count(*) FROM synthetic", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn open_flags_path_timeout_and_schema_pragmas_follow_sqlite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local.db");
    assert!(Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).is_err());
    assert!(!path.exists());
    let connection = Connection::open(&path).unwrap();
    let canonical_path = path.canonicalize().unwrap();
    assert_eq!(connection.path(), canonical_path.to_str());
    connection.busy_timeout(Duration::from_millis(17)).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "busy_timeout", |row| row.get::<_, u32>(0))
            .unwrap(),
        17
    );
    connection.pragma_update(None, "user_version", 42).unwrap();
    connection
        .execute_batch("CREATE TABLE synthetic(value); ATTACH ':memory:' AS other")
        .unwrap();
    connection
        .pragma_update(Some("other"), "user_version", 7)
        .unwrap();
    for (schema, expected) in [(None, 42), (Some("main"), 42), (Some("other"), 7)] {
        assert_eq!(
            connection
                .pragma_query_value(schema, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            expected
        );
    }
    let read_only = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(matches!(
        read_only.execute("INSERT INTO synthetic VALUES(1)", []),
        Err(Error::SqliteFailure(error, _)) if error.code == ErrorCode::ReadOnly
    ));
}

#[test]
fn typed_parameters_and_prepared_statement_reuse_keep_exact_values() {
    let connection = memory();
    connection
        .execute_batch("CREATE TABLE synthetic(id INTEGER PRIMARY KEY, text TEXT, blob BLOB, optional INTEGER, boolean INTEGER)")
        .unwrap();
    {
        let mut statement = connection
            .prepare("INSERT INTO synthetic VALUES(?1,?2,?3,?4,?5)")
            .unwrap();
        assert_eq!(
            statement
                .execute(params![
                    i64::MAX,
                    "会社\0🦀",
                    vec![0_u8, 255],
                    None::<i64>,
                    true
                ])
                .unwrap(),
            1
        );
        statement
            .execute((i64::MIN, "second", vec![7_u8], Some(9), false))
            .unwrap();
        assert!(matches!(
            statement.execute([1]),
            Err(Error::InvalidParameterCount(1, 5))
        ));
    }
    for id in [i64::MAX, i64::MIN, i64::MAX] {
        let mut statement = connection
            .prepare_cached("SELECT * FROM synthetic WHERE id=?1")
            .unwrap();
        assert_eq!(statement.column_count(), 5);
        let values: Vec<Value> = statement
            .query_row(params_from_iter([id]), |row| {
                (0..5).map(|index| row.get(index)).collect()
            })
            .unwrap();
        if id == i64::MAX {
            assert_eq!(
                values,
                vec![
                    Value::Integer(id),
                    Value::Text("会社\0🦀".into()),
                    Value::Blob(vec![0, 255]),
                    Value::Null,
                    Value::Integer(1)
                ]
            );
        } else {
            assert_eq!(
                values,
                vec![
                    Value::Integer(id),
                    Value::Text("second".into()),
                    Value::Blob(vec![7]),
                    Value::Integer(9),
                    Value::Integer(0)
                ]
            );
        }
    }
    assert_eq!(connection.last_insert_rowid(), i64::MIN);
}

#[test]
fn execute_batch_keeps_trigger_bodies_and_sqlite_errors() {
    let connection = memory();
    connection
        .execute_batch("CREATE TABLE synthetic(value INTEGER UNIQUE); CREATE TABLE events(value TEXT); CREATE TRIGGER record AFTER INSERT ON synthetic BEGIN INSERT INTO events VALUES('first;second'); INSERT INTO events VALUES('third'); END; INSERT INTO synthetic VALUES(1)")
        .unwrap();
    assert_eq!(count(&connection), 1);
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        matches!(connection.execute("INSERT INTO synthetic VALUES(1)", []), Err(Error::SqliteFailure(error, _)) if error.code == ErrorCode::ConstraintViolation)
    );
    assert!(matches!(
        connection.execute("SELECT 1", []),
        Err(Error::ExecuteReturnedResults)
    ));
    assert!(connection.prepare("SELECT FROM broken").is_err());
}

fn named_integer(row: &Row<'_>, name: &str) -> Result<i64> {
    row.get(name)
}

#[test]
fn rows_next_uses_borrowed_names_and_resets_on_early_drop() {
    let connection = memory();
    let mut statement = connection
        .prepare("SELECT ?1 AS Name, x'00FF' AS blob UNION ALL SELECT 2, x'07'")
        .unwrap();
    let name = String::from("nAmE");
    {
        let mut rows = statement.query([i64::MAX]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(named_integer(row, name.as_str()).unwrap(), i64::MAX);
        assert_eq!(row.get::<_, Vec<u8>>("blob").unwrap(), vec![0, 255]);
        assert_eq!(
            row.get::<_, i64>("missing"),
            Err(Error::InvalidColumnName("missing".into()))
        );
    }
    let mut rows = statement.query([17]).unwrap();
    assert_eq!(rows.next().unwrap().unwrap().get::<_, i64>(0).unwrap(), 17);
    assert_eq!(rows.next().unwrap().unwrap().get::<_, i64>(0).unwrap(), 2);
    assert!(rows.next().unwrap().is_none());
    assert!(rows.next().unwrap().is_none());
}

#[test]
fn query_map_is_lazy_continues_after_conversion_error_and_releases_statement() {
    let connection = memory();
    let mut statement = connection
        .prepare("SELECT 1 AS value UNION ALL SELECT 'wrong' UNION ALL SELECT 3")
        .unwrap();
    let calls = Cell::new(0);
    {
        let mut rows = statement
            .query_map([], |row| {
                calls.set(calls.get() + 1);
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(calls.get(), 0);
        assert_eq!(rows.next().unwrap().unwrap(), 1);
        assert!(matches!(
            rows.next(),
            Some(Err(Error::InvalidColumnType(0, _, _)))
        ));
        assert_eq!(rows.next().unwrap().unwrap(), 3);
        assert!(rows.next().is_none());
    }
    assert_eq!(calls.get(), 3);
    let values = statement
        .query_map([], |row| row.get::<_, Value>(0))
        .unwrap()
        .collect::<Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        values,
        vec![
            Value::Integer(1),
            Value::Text("wrong".into()),
            Value::Integer(3)
        ]
    );
}

#[test]
fn query_row_exists_and_optional_preserve_not_found_and_conversion_errors() {
    let connection = memory();
    let mut statement = connection.prepare("SELECT ?1 AS value WHERE ?1>0").unwrap();
    assert!(!statement.exists([0]).unwrap());
    assert!(statement.exists([1]).unwrap());
    assert_eq!(
        statement
            .query_row([0], |row| row.get::<_, i64>(0))
            .optional()
            .unwrap(),
        None
    );
    assert_eq!(
        statement
            .query_row([2], |row| row.get::<_, i64>(0))
            .optional()
            .unwrap(),
        Some(2)
    );
    assert!(matches!(
        statement
            .query_row([2], |row| row.get::<_, String>(0))
            .optional(),
        Err(Error::InvalidColumnType(0, _, _))
    ));
    assert_eq!(
        connection.query_row("SELECT 1 WHERE 0", [], |row| row.get::<_, i64>(0)),
        Err(Error::QueryReturnedNoRows)
    );
    assert_eq!(
        connection
            .query_row("SELECT 1 UNION ALL SELECT 2", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        connection
            .query_row("SELECT * FROM missing", [], |row| row.get::<_, i64>(0))
            .optional()
            .is_err()
    );
}

#[test]
fn callbacks_borrow_local_rows_and_reject_only_conversions_they_read() {
    let connection = memory();
    assert_eq!(
        connection
            .query_row("SELECT 9e999, 7 AS value", [], |row| row
                .get::<_, i64>("value"))
            .unwrap(),
        7
    );
    let mut statement = connection
        .prepare("SELECT CAST(x'FF' AS TEXT), 7 AS value")
        .unwrap();
    assert_eq!(
        statement
            .query_map([], |row| row.get::<_, i64>("value"))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap(),
        vec![7]
    );
    assert!(matches!(
        statement.query_row([], |row| row.get::<_, String>(0)),
        Err(Error::Utf8Error(0, _))
    ));
}

#[test]
fn rows_next_preserves_native_projection_of_unused_invalid_cells() {
    let connection = memory();
    let native = rusqlite::Connection::open_in_memory().unwrap();
    let name = String::from("value");
    for sql in [
        "SELECT CAST(x'FF' AS TEXT) AS skipped, 7 AS value",
        "SELECT 9e999 AS skipped, 7 AS value",
    ] {
        let expected = native
            .query_row(sql, [], |row| row.get::<_, i64>(name.as_str()))
            .unwrap();
        let mut statement = connection.prepare(sql).unwrap();
        {
            let mut rows = statement.query([]).unwrap();
            let row = rows.next().unwrap().unwrap();
            assert_eq!(row.get::<_, i64>(name.as_str()).unwrap(), expected);
            assert!(rows.next().unwrap().is_none());
        }
        assert_eq!(
            statement
                .query_row([], |row| row.get::<_, i64>(name.as_str()))
                .unwrap(),
            expected
        );
        assert_eq!(
            statement
                .query_map([], |row| row.get::<_, i64>(name.as_str()))
                .unwrap()
                .collect::<Result<Vec<_>>>()
                .unwrap(),
            vec![expected]
        );
    }
}

#[test]
fn buffered_local_infinity_matches_native_for_skipped_and_requested_columns() {
    let connection = memory();
    let native = rusqlite::Connection::open_in_memory().unwrap();
    for sql in [
        "SELECT 9e999 AS real, 7 AS value",
        "SELECT -9e999 AS real, 7 AS value",
    ] {
        let expected = native
            .query_row(sql, [], |row| row.get::<_, f64>("real"))
            .unwrap();
        let mut statement = connection.prepare(sql).unwrap();
        let mut rows = statement.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(row.get::<_, i64>("value").unwrap(), 7);
        assert_eq!(row.get::<_, f64>("real").unwrap(), expected);
        assert_eq!(
            connection
                .query_row(sql, [], |row| row.get::<_, f64>("real"))
                .unwrap(),
            expected
        );
    }
}

#[test]
fn buffered_local_cells_defer_utf8_validation_and_keep_native_infinity() {
    let connection = memory();
    let native = rusqlite::Connection::open_in_memory().unwrap();
    let sql = "SELECT 7 AS value, CAST(x'FF' AS TEXT) AS bad, 9e999 AS real, x'FF00' AS blob";
    let mut native_statement = native.prepare(sql).unwrap();
    let mut native_rows = native_statement.query([]).unwrap();
    let expected = native_rows.next().unwrap().unwrap();
    let mut statement = connection.prepare(sql).unwrap();
    let mut rows = statement.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    let bad = String::from("bad");
    for index in 0..4 {
        assert_eq!(row.get_ref(index), expected.get_ref(index));
    }
    assert_eq!(
        row.get::<_, String>(bad.as_str()),
        expected.get::<_, String>(bad.as_str())
    );
    assert!(matches!(
        row.get::<_, String>(1),
        Err(Error::Utf8Error(1, _))
    ));
    assert_eq!(row.get::<_, f64>("real"), expected.get::<_, f64>("real"));
    assert_eq!(row.get::<_, i64>(2), expected.get::<_, i64>(2));
    assert_eq!(row.get::<_, Vec<u8>>(3), expected.get::<_, Vec<u8>>(3));
    assert_eq!(
        connection.query_row(sql, [], |row| row.get::<_, f64>("real")),
        expected.get::<_, f64>("real")
    );
    assert_eq!(
        connection
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get::<_, f64>("real"))
            .unwrap()
            .collect::<Result<Vec<_>>>()
            .unwrap(),
        vec![f64::INFINITY]
    );
}

#[test]
fn unchecked_transaction_shares_original_connection_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .unwrap();
    connection
        .execute_batch("CREATE TABLE synthetic(value); INSERT INTO synthetic VALUES(1)")
        .unwrap();
    let other = Connection::open(&path).unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    assert!(!connection.is_autocommit());
    assert_eq!(count(&transaction), 1);
    other
        .execute("INSERT INTO synthetic VALUES(2)", [])
        .unwrap();
    assert_eq!(count(&connection), 1);
    assert!(connection.unchecked_transaction().is_err());
    assert!(!connection.is_autocommit());
    transaction.rollback().unwrap();
    assert!(connection.is_autocommit());
    assert_eq!(count(&connection), 2);
}

#[test]
fn transaction_commit_explicit_rollback_and_drop_keep_atomic_changes() {
    let mut connection = memory();
    connection
        .execute_batch("CREATE TABLE synthetic(id INTEGER PRIMARY KEY, value TEXT)")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    assert!(!transaction.is_autocommit());
    transaction
        .execute("INSERT INTO synthetic(value) VALUES(?1)", ["commit"])
        .unwrap();
    assert_eq!(transaction.last_insert_rowid(), 1);
    transaction.commit().unwrap();
    assert!(connection.is_autocommit());
    assert_eq!(count(&connection), 1);
    let transaction = connection.transaction().unwrap();
    transaction
        .execute("INSERT INTO synthetic(value) VALUES(?1)", ["rollback"])
        .unwrap();
    transaction.rollback().unwrap();
    assert_eq!(count(&connection), 1);
    {
        let transaction = connection.transaction().unwrap();
        transaction.execute("DELETE FROM synthetic", []).unwrap();
        assert_eq!(count(&transaction), 0);
    }
    assert!(connection.is_autocommit());
    assert_eq!(count(&connection), 1);
}

#[test]
fn transaction_behavior_acquires_the_expected_write_lock() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lock.db");
    let mut connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE synthetic(value)")
        .unwrap();
    let mut other = Connection::open(&path).unwrap();
    other.busy_timeout(Duration::ZERO).unwrap();
    let deferred = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .unwrap();
    other
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap()
        .rollback()
        .unwrap();
    deferred.rollback().unwrap();
    for behavior in [
        TransactionBehavior::Immediate,
        TransactionBehavior::Exclusive,
    ] {
        let transaction = connection.transaction_with_behavior(behavior).unwrap();
        assert!(
            matches!(other.execute("INSERT INTO synthetic VALUES(1)", []), Err(Error::SqliteFailure(error, _)) if error.code == ErrorCode::DatabaseBusy)
        );
        transaction.commit().unwrap();
        other
            .execute("INSERT INTO synthetic VALUES(1)", [])
            .unwrap();
    }
}

#[test]
fn failed_commit_rolls_back_deferred_constraints_like_native_sqlite() {
    let mut connection = memory();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE synthetic(parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED)").unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute("INSERT INTO synthetic VALUES(9)", [])
        .unwrap();
    assert_eq!(count(&transaction), 1);
    assert!(
        matches!(transaction.commit(), Err(Error::SqliteFailure(error, _)) if error.code == ErrorCode::ConstraintViolation)
    );
    assert!(connection.is_autocommit());
    assert_eq!(count(&connection), 0);
    connection.transaction().unwrap().commit().unwrap();
}

#[test]
fn sqlite_automatic_rollback_leaves_drop_safe_and_connection_reusable() {
    let mut connection = memory();
    connection
        .execute_batch("CREATE TABLE synthetic(value INTEGER UNIQUE)")
        .unwrap();
    {
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("INSERT INTO synthetic VALUES(1)", [])
            .unwrap();
        assert!(
            transaction
                .execute("INSERT OR ROLLBACK INTO synthetic VALUES(1)", [])
                .is_err()
        );
        assert!(transaction.is_autocommit());
    }
    assert_eq!(count(&connection), 0);
    connection.transaction().unwrap().commit().unwrap();
}

#[test]
fn backup_reads_uncheckpointed_wal_and_restore_recreates_typed_contents() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.db");
    let backup = directory.path().join("backup.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .unwrap();
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    connection.execute_batch("CREATE TABLE synthetic(value INTEGER, text TEXT, blob BLOB); INSERT INTO synthetic VALUES(1, 'first', NULL)").unwrap();
    connection
        .execute(
            "INSERT INTO synthetic VALUES(?1, ?2, ?3)",
            params![i64::MAX, "会社🦀", vec![0_u8, 255]],
        )
        .unwrap();
    assert!(
        path.with_file_name("source.db-wal")
            .metadata()
            .unwrap()
            .len()
            > 0
    );
    connection.backup(MAIN_DB, &backup, None).unwrap();
    let saved = Connection::open_with_flags(&backup, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(count(&saved), 2);
    let mut restored = Connection::open(directory.path().join("new.db")).unwrap();
    let calls = Cell::new(0);
    restored
        .restore(
            MAIN_DB,
            &backup,
            Some(|progress: meibo_db::backup::Progress| {
                calls.set(calls.get() + 1);
                assert!(progress.remaining >= 0);
                assert!(progress.pagecount >= progress.remaining);
            }),
        )
        .unwrap();
    assert!(calls.get() > 0);
    for copy in [&saved, &restored] {
        assert_eq!(
            copy.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            copy.query_row(
                "SELECT value, text, blob FROM synthetic ORDER BY value DESC",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?
                ))
            )
            .unwrap(),
            (i64::MAX, "会社🦀".into(), vec![0, 255])
        );
    }
    restored
        .restore(MAIN_DB, &backup, None::<fn(meibo_db::backup::Progress)>)
        .unwrap();
    assert_eq!(count(&restored), 2);
}
