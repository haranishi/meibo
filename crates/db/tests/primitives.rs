//! Compatibility checks for the adapter's typed SQL boundary.

use std::{io, sync::Arc};

use meibo_db::{
    Error, ErrorCode, FromSql, OptionalExtension, Params, Result, Row, ToSql, Value, ValueRef,
    params, params_from_iter,
    types::{FromSqlError, FromSqlResult, ToSqlOutput, Type},
};

fn columns(names: &[&str]) -> Arc<[String]> {
    names.iter().map(ToString::to_string).collect()
}

fn values<P: Params>(params: P) -> Result<Vec<Value>> {
    params.into_values()
}

#[test]
fn integer_extremes_never_pass_through_float() {
    let expected = [
        i64::MIN,
        -9_007_199_254_740_993,
        9_007_199_254_740_993,
        i64::MAX,
    ];
    let row = Row::from_values(
        columns(&["min", "negative", "positive", "max"]),
        values(params_from_iter(expected)).unwrap(),
    )
    .unwrap();
    for (index, integer) in expected.into_iter().enumerate() {
        assert_eq!(row.get::<_, i64>(index).unwrap(), integer);
        assert_eq!(row.get::<_, Value>(index).unwrap(), Value::Integer(integer));
        assert_eq!(row.get_ref(index).unwrap(), ValueRef::Integer(integer));
    }
}

#[test]
fn nullable_boolean_blob_and_unicode_text_match_sqlite() {
    let text = "営業\0会社🦀";
    let blob = vec![0, 0xff, 0x80, 1];
    let row = Row::from_values(
        columns(&["nil", "text", "blob", "yes", "no", "nonzero"]),
        values(params![None::<String>, text, blob, true, false, -12]).unwrap(),
    )
    .unwrap();
    assert_eq!(row.get::<_, Option<String>>(0).unwrap(), None);
    assert_eq!(row.get::<_, Option<i64>>(0).unwrap(), None);
    assert_eq!(row.get::<_, Option<Vec<u8>>>(0).unwrap(), None);
    assert_eq!(row.get::<_, Option<bool>>(0).unwrap(), None);
    assert_eq!(row.get::<_, Value>(0).unwrap(), Value::Null);
    assert_eq!(row.get::<_, Option<String>>(1).unwrap(), Some(text.into()));
    assert_eq!(row.get::<_, Vec<u8>>(2).unwrap(), blob);
    assert!(row.get::<_, bool>(3).unwrap());
    assert!(!row.get::<_, bool>(4).unwrap());
    assert!(row.get::<_, bool>(5).unwrap());
}

#[test]
fn finite_reals_keep_their_bits_including_negative_zero() {
    let expected = [f64::MIN, -0.0, 0.0, f64::MIN_POSITIVE, 1.5, f64::MAX];
    for value in expected {
        let row = Row::from_values(columns(&["real"]), values([value]).unwrap()).unwrap();
        assert_eq!(row.get::<_, f64>(0).unwrap().to_bits(), value.to_bits());
    }
}

#[test]
fn non_finite_values_are_rejected_before_owned_rows_or_parameters() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for real in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(values([real]).is_err());
        assert!(values(params![Value::Real(real)]).is_err());
        assert!(Row::from_values(columns(&["real"]), vec![Value::Real(real)]).is_err());
    }
    connection
        .query_row("SELECT 9e999", [], |raw| {
            let row = Row::from_local(raw);
            assert_eq!(row.get_ref(0), raw.get_ref(0));
            assert_eq!(row.get::<_, f64>(0), raw.get::<_, f64>(0));
            Ok(())
        })
        .unwrap();
}

fn compare_get<T: FromSql + PartialEq + std::fmt::Debug>(
    local: &rusqlite::Row<'_>,
    owned: &Row<'_>,
    index: usize,
) {
    assert_eq!(owned.get::<_, T>(index), local.get::<_, T>(index));
}

#[test]
fn owned_and_local_rows_follow_actual_sqlite_conversions() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let mut statement = connection
        .prepare("SELECT ?1 AS Int, ?2 AS real, ?3 AS text, ?4 AS blob, NULL AS nil, -2 AS boolean")
        .unwrap();
    let input = vec![
        Value::Integer(i64::MAX),
        Value::Real(1.25),
        Value::Text("会社\0🦀".into()),
        Value::Blob(vec![0, 255]),
    ];
    let mut rows = statement.query(rusqlite::params_from_iter(&input)).unwrap();
    let local = rows.next().unwrap().unwrap();
    let mut output = input;
    output.extend([Value::Null, Value::Integer(-2)]);
    let owned = Row::from_values(
        columns(&["Int", "real", "text", "blob", "nil", "boolean"]),
        output,
    )
    .unwrap();
    let forwarded = Row::from_local(local);
    for row in [&owned, &forwarded] {
        compare_get::<i64>(local, row, 0);
        compare_get::<f64>(local, row, 0);
        compare_get::<f64>(local, row, 1);
        compare_get::<String>(local, row, 2);
        compare_get::<Vec<u8>>(local, row, 3);
        compare_get::<Option<String>>(local, row, 4);
        compare_get::<bool>(local, row, 5);
        compare_get::<i8>(local, row, 0);
        compare_get::<u8>(local, row, 5);
        compare_get::<String>(local, row, 0);
        compare_get::<i64>(local, row, 1);
        compare_get::<i64>(local, row, 2);
        compare_get::<String>(local, row, 3);
        compare_get::<String>(local, row, 4);
        compare_get::<bool>(local, row, 2);
        compare_get::<Option<String>>(local, row, 0);
        assert_eq!(row.get::<_, i64>("INT").unwrap(), i64::MAX);
        assert_eq!(
            row.get::<_, String>("missing"),
            local.get::<_, String>("missing")
        );
        assert_eq!(row.get::<_, i64>(6), local.get::<_, i64>(6));
        assert_eq!(row.get_ref(usize::MAX), local.get_ref(usize::MAX));
    }
}

#[test]
fn column_names_match_ascii_case_and_choose_the_first_duplicate() {
    let row = Row::from_values(
        columns(&["Name", "NAME", "会社", "Ä"]),
        values([10, 20, 30, 40]).unwrap(),
    )
    .unwrap();
    assert_eq!(row.get::<_, i64>("name").unwrap(), 10);
    assert_eq!(row.get::<_, i64>("会社").unwrap(), 30);
    assert_eq!(row.get::<_, i64>("Ä").unwrap(), 40);
    assert_eq!(
        row.get::<_, i64>("ä"),
        Err(Error::InvalidColumnName("ä".into()))
    );
    assert_eq!(row.get_ref(4), Err(Error::InvalidColumnIndex(4)));
}

#[test]
fn malformed_row_width_is_rejected() {
    assert!(Row::from_values(columns(&["a", "b"]), vec![Value::Null]).is_err());
    assert!(Row::from_values(columns(&["a"]), vec![Value::Null, Value::Null]).is_err());
    let empty = Row::from_values(columns(&[]), vec![]).unwrap();
    assert_eq!(empty.get::<_, i64>(0), Err(Error::InvalidColumnIndex(0)));
}

#[test]
fn custom_from_sql_errors_keep_index_type_and_cause() {
    #[derive(Debug)]
    struct Rejected;
    impl FromSql for Rejected {
        fn column_result(_: ValueRef<'_>) -> FromSqlResult<Self> {
            Err(FromSqlError::other(io::Error::other("conversion rejected")))
        }
    }
    let row = Row::from_values(
        columns(&["id", "data"]),
        values(params![1, vec![7_u8]]).unwrap(),
    )
    .unwrap();
    assert!(
        matches!(row.get::<_, Rejected>(1), Err(Error::FromSqlConversionFailure(1, Type::Blob, cause)) if cause.to_string() == "conversion rejected")
    );
    assert!(matches!(
        row.get::<_, [u8; 2]>(1),
        Err(Error::FromSqlConversionFailure(1, Type::Blob, _))
    ));
}

#[test]
fn borrowed_local_invalid_utf8_is_a_conversion_error() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .query_row("SELECT CAST(x'FF' AS TEXT)", [], |raw| {
            let row = Row::from_local(raw);
            assert_eq!(row.get::<_, String>(0), raw.get::<_, String>(0));
            assert!(matches!(
                row.get::<_, String>(0),
                Err(Error::Utf8Error(0, _))
            ));
            Ok(())
        })
        .unwrap();
}

#[test]
fn params_support_macro_iterators_arrays_slices_and_used_tuples() {
    assert_eq!(values([]).unwrap(), Vec::<Value>::new());
    assert_eq!(values(()).unwrap(), Vec::<Value>::new());
    assert_eq!(values(params![]).unwrap(), Vec::<Value>::new());
    assert_eq!(
        values([1, 2]).unwrap(),
        vec![Value::Integer(1), Value::Integer(2)]
    );
    let referenced = &[&1_i64, &2_i64];
    assert_eq!(
        values(referenced).unwrap(),
        vec![Value::Integer(1), Value::Integer(2)]
    );
    assert_eq!(
        values(["a", "会社"]).unwrap(),
        vec![Value::Text("a".into()), Value::Text("会社".into())]
    );
    assert_eq!(
        values((i64::MAX, true)).unwrap(),
        vec![Value::Integer(i64::MAX), Value::Integer(1)]
    );
    assert_eq!(
        values(("single",)).unwrap(),
        vec![Value::Text("single".into())]
    );
    let text = String::from("owned");
    let dynamic: &[&dyn ToSql] = &[&text, &None::<i64>];
    assert_eq!(
        values(dynamic).unwrap(),
        vec![Value::Text(text.clone()), Value::Null]
    );
    let input = vec![Value::Integer(i64::MIN), Value::Text(text), Value::Null];
    assert_eq!(values(params_from_iter(input.iter())).unwrap(), input);
    assert_eq!(values(params_from_iter(input.clone())).unwrap(), input);
}

#[test]
fn tuple_parameters_preserve_existing_erasure_and_list_update_shapes() {
    let erased_key = String::from("(消去済ミ)");
    let erased_company_name = "(消去済み)";
    let company_id = i64::MAX;
    let at = 1_794_015_600_i64;
    assert_eq!(
        values((company_id, at, erased_company_name, &erased_key)).unwrap(),
        vec![
            Value::Integer(company_id),
            Value::Integer(at),
            Value::Text(erased_company_name.into()),
            Value::Text(erased_key.clone()),
        ]
    );
    assert_eq!(
        values((i64::MIN, true, 5_u8, at, Some(12_i64))).unwrap(),
        vec![
            Value::Integer(i64::MIN),
            Value::Integer(1),
            Value::Integer(5),
            Value::Integer(at),
            Value::Integer(12),
        ]
    );
    assert_eq!(
        values((5_u8, false, 23_i64)).unwrap(),
        vec![Value::Integer(5), Value::Integer(0), Value::Integer(23)]
    );
}

#[test]
fn params_macro_borrows_and_evaluates_each_expression_once() {
    let text = String::from("会社");
    let blob = vec![1_u8, 0, 255];
    let mut evaluations = 0;
    let converted = values(params![
        text,
        blob,
        {
            evaluations += 1;
            i64::MAX
        },
        Some(true),
    ])
    .unwrap();
    assert_eq!(evaluations, 1);
    assert_eq!(text, "会社");
    assert_eq!(blob, [1, 0, 255]);
    assert_eq!(
        converted,
        vec![
            Value::Text(text),
            Value::Blob(blob),
            Value::Integer(i64::MAX),
            Value::Integer(1)
        ]
    );
}

#[test]
fn to_sql_failures_and_invalid_utf8_are_returned_without_panics() {
    struct Rejected;
    impl ToSql for Rejected {
        fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
            Err(Error::ToSqlConversionFailure(Box::new(io::Error::other(
                "conversion rejected",
            ))))
        }
    }
    struct InvalidText;
    impl ToSql for InvalidText {
        fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
            Ok(ToSqlOutput::Borrowed(ValueRef::Text(&[255])))
        }
    }
    assert!(
        matches!(values(params![Rejected]), Err(Error::ToSqlConversionFailure(cause)) if cause.to_string() == "conversion rejected")
    );
    assert!(matches!(
        values(params![InvalidText]),
        Err(Error::ToSqlConversionFailure(_))
    ));
}

#[test]
fn optional_only_swallows_no_rows_and_preserves_local_error_codes() {
    assert_eq!(Ok::<_, Error>(7).optional().unwrap(), Some(7));
    assert_eq!(
        Err::<i64, _>(Error::QueryReturnedNoRows)
            .optional()
            .unwrap(),
        None
    );
    assert_eq!(
        Err::<i64, _>(Error::InvalidQuery).optional(),
        Err(Error::InvalidQuery)
    );
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE item(id INTEGER PRIMARY KEY)")
        .unwrap();
    connection
        .execute("INSERT INTO item VALUES (1)", [])
        .unwrap();
    let duplicate: Error = connection
        .execute("INSERT INTO item VALUES (1)", [])
        .unwrap_err();
    assert_eq!(
        duplicate.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
    assert!(Err::<i64, _>(duplicate).optional().is_err());
    let missing: Result<i64> =
        connection.query_row("SELECT id FROM item WHERE id=2", [], |r| r.get(0));
    assert_eq!(missing.optional().unwrap(), None);
}
