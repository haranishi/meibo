use super::{
    MAX_COLUMNS, MAX_ROW_BYTES, SnapshotError, crm_table, crm_tables, decode_crm_sequences,
    decode_row, encode_crm_sequences, encode_row, read_crm_sequences,
};
use crate::{
    OpenOptions,
    schema::Db,
    sql::{params, types::Value},
    types::DataOrigin,
};

fn database() -> (tempfile::TempDir, Db) {
    let directory = tempfile::tempdir().unwrap();
    let options = OpenOptions {
        cache_mib: 8,
        background_checkpoint: false,
        ..OpenOptions::default()
    };
    let db = Db::create(
        &directory.path().join("snapshot.db"),
        DataOrigin::Synthetic,
        &options,
    )
    .unwrap();
    (directory, db)
}

fn same_values(actual: &[Value], expected: &[Value]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        match (actual, expected) {
            (Value::Real(actual), Value::Real(expected)) => {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
            _ => assert_eq!(actual, expected),
        }
    }
}

#[test]
fn typed_row_has_the_exact_canonical_wire_representation() {
    let values = [
        Value::Integer(42),
        Value::Null,
        Value::Real(-0.0),
        Value::Text("日本語".into()),
        Value::Blob(vec![0, 1, 255]),
    ];
    let bytes = encode_row(&values).unwrap();
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        r#"{"version":1,"values":[{"type":"integer","value":"42"},{"type":"null"},{"type":"real","value":"8000000000000000"},{"type":"text","value":"日本語"},{"type":"blob","value":"AAH/"}]}"#
    );
    same_values(&decode_row(&bytes, values.len()).unwrap(), &values);
}

#[test]
fn integers_never_pass_through_a_floating_point_value() {
    let values = [i64::MIN, -1, 0, 9_007_199_254_740_993, i64::MAX].map(Value::Integer);
    let bytes = encode_row(&values).unwrap();
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("9007199254740993")
    );
    same_values(&decode_row(&bytes, values.len()).unwrap(), &values);
}

#[test]
fn finite_real_bits_and_signed_zero_round_trip_exactly() {
    let values = [
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::MAX,
        -f64::MAX,
    ]
    .map(Value::Real);
    let bytes = encode_row(&values).unwrap();
    same_values(&decode_row(&bytes, values.len()).unwrap(), &values);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            encode_row(&[Value::Real(value)]),
            Err(SnapshotError::InvalidRow)
        );
    }
}

#[test]
fn blobs_use_padded_standard_base64_and_preserve_every_byte() {
    for (blob, encoded) in [
        (&b""[..], ""),
        (&b"\x01"[..], "AQ=="),
        (&b"\x01\x02"[..], "AQI="),
    ] {
        let expected =
            format!("{{\"version\":1,\"values\":[{{\"type\":\"blob\",\"value\":\"{encoded}\"}}]}}");
        assert_eq!(
            encode_row(&[Value::Blob(blob.to_vec())]).unwrap(),
            expected.as_bytes()
        );
    }
    let values = [Value::Blob((0..=255).collect())];
    same_values(
        &decode_row(&encode_row(&values).unwrap(), 1).unwrap(),
        &values,
    );
    for encoded in ["AQ", "AR==", "_w==", "AQ===", "!!=="] {
        let bytes =
            format!("{{\"version\":1,\"values\":[{{\"type\":\"blob\",\"value\":\"{encoded}\"}}]}}");
        assert_eq!(
            decode_row(bytes.as_bytes(), 1),
            Err(SnapshotError::InvalidRow)
        );
    }
}

#[test]
fn text_preserves_unicode_control_characters_and_escaping() {
    let values = [Value::Text(
        "日本語 e\u{301} é \" \\ \n \t \u{0} \u{2028}".into(),
    )];
    let bytes = encode_row(&values).unwrap();
    assert!(std::str::from_utf8(&bytes).unwrap().contains("\\u0000"));
    same_values(&decode_row(&bytes, 1).unwrap(), &values);
}

#[test]
fn invalid_types_versions_and_noncanonical_scalar_representations_fail() {
    let cases = [
        r#"{"values":[{"type":"null"}]}"#,
        r#"{"version":2,"values":[{"type":"null"}]}"#,
        r#"{"version":1.0,"values":[{"type":"null"}]}"#,
        r#"{"version":1,"values":[{"type":"unknown"}]}"#,
        r#"{"version":1,"values":[{"type":"null","value":null}]}"#,
        r#"{"version":1,"values":[{"type":"integer","value":42}]}"#,
        r#"{"version":1,"values":[{"type":"integer","value":"01"}]}"#,
        r#"{"version":1,"values":[{"type":"integer","value":"+1"}]}"#,
        r#"{"version":1,"values":[{"type":"integer","value":"-0"}]}"#,
        r#"{"version":1,"values":[{"type":"integer","value":"9223372036854775808"}]}"#,
        r#"{"version":1,"values":[{"type":"real","value":"7ff0000000000000"}]}"#,
        r#"{"version":1,"values":[{"type":"real","value":"7ff8000000000000"}]}"#,
        r#"{"version":1,"values":[{"type":"real","value":"3FF0000000000000"}]}"#,
        r#"{"version":1,"values":[{"type":"real","value":"0"}]}"#,
        r#"{"version":1,"values":[{"type":"text","value":null}]}"#,
    ];
    for bytes in cases {
        assert_eq!(
            decode_row(bytes.as_bytes(), 1),
            Err(SnapshotError::InvalidRow)
        );
    }
}

#[test]
fn duplicate_unknown_reordered_and_noncanonical_json_fields_fail() {
    let cases = [
        r#"{"version":1,"version":1,"values":[{"type":"null"}]}"#,
        r#"{"version":1,"values":[{"type":"null"}],"extra":"private"}"#,
        r#"{"values":[{"type":"null"}],"version":1}"#,
        r#"{"version":1,"values":[{"type":"text","type":"text","value":"x"}]}"#,
        r#"{"version":1,"values":[{"type":"text","value":"x","value":"x"}]}"#,
        r#"{"version":1,"values":[{"value":"x","type":"text"}]}"#,
        r#"{"version":1,"values":[{"type":"null","extra":0}]}"#,
        r#"{ "version":1,"values":[{"type":"null"}]}"#,
        r#"{"version":1,"values":[{"type":"text","value":"\u65e5"}]}"#,
        r#"{"version":1,"values":[{"type":"null"}]}{}"#,
    ];
    for bytes in cases {
        assert_eq!(
            decode_row(bytes.as_bytes(), 1),
            Err(SnapshotError::InvalidRow)
        );
    }
}

#[test]
fn exact_row_byte_limit_is_accepted_and_overflow_is_rejected() {
    let overhead = br#"{"version":1,"values":[{"type":"text","value":""}]}"#.len();
    let values = [Value::Text("x".repeat(MAX_ROW_BYTES - overhead))];
    let bytes = encode_row(&values).unwrap();
    assert_eq!(bytes.len(), MAX_ROW_BYTES);
    same_values(&decode_row(&bytes, 1).unwrap(), &values);
    assert_eq!(
        encode_row(&[Value::Text("x".repeat(MAX_ROW_BYTES - overhead + 1))]),
        Err(SnapshotError::LimitExceeded)
    );
    assert_eq!(
        decode_row(&vec![b'!'; MAX_ROW_BYTES + 1], 1),
        Err(SnapshotError::LimitExceeded)
    );
    assert_eq!(
        encode_row(&[Value::Blob(vec![0; MAX_ROW_BYTES])]),
        Err(SnapshotError::LimitExceeded)
    );
    assert_eq!(
        encode_row(&[Value::Text("\u{0}".repeat(MAX_ROW_BYTES / 6 + 1))]),
        Err(SnapshotError::LimitExceeded)
    );
}

#[test]
fn column_limits_precede_capacity_allocation_and_match_the_trusted_profile() {
    assert_eq!(
        decode_row(b"{}", usize::MAX),
        Err(SnapshotError::LimitExceeded)
    );
    assert_eq!(decode_row(b"{}", 0), Err(SnapshotError::LimitExceeded));
    assert_eq!(encode_row(&[]), Err(SnapshotError::LimitExceeded));
    assert_eq!(
        encode_row(&vec![Value::Null; MAX_COLUMNS + 1]),
        Err(SnapshotError::LimitExceeded)
    );
    let values = vec![Value::Null; MAX_COLUMNS];
    same_values(
        &decode_row(&encode_row(&values).unwrap(), MAX_COLUMNS).unwrap(),
        &values,
    );
    assert_eq!(
        decode_row(br#"{"version":1,"values":[{"type":"null"}]}"#, 2),
        Err(SnapshotError::InvalidRow)
    );
}

#[test]
fn excessive_columns_and_nested_structure_do_not_collect_untrusted_trees() {
    let bytes = format!(
        "{{\"version\":1,\"values\":[{}]}}",
        vec![r#"{"type":"null"}"#; 40_000].join(",")
    );
    assert!(bytes.len() < MAX_ROW_BYTES);
    assert_eq!(
        decode_row(bytes.as_bytes(), 1),
        Err(SnapshotError::LimitExceeded)
    );
    let bytes = format!(
        "{{\"version\":1,\"values\":[{}0{}]}}",
        "[".repeat(200),
        "]".repeat(200)
    );
    assert_eq!(
        decode_row(bytes.as_bytes(), 1),
        Err(SnapshotError::InvalidRow)
    );
}

#[test]
fn registry_matches_actual_physical_columns_and_primary_key_order() {
    let (_directory, db) = database();
    let mut names = db.conn().prepare(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'name_fts%' AND name<>'erase_gate' ORDER BY name"
    ).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<crate::sql::Result<Vec<_>>>().unwrap();
    let mut registered = crm_tables()
        .iter()
        .map(|table| table.name().to_owned())
        .collect::<Vec<_>>();
    registered.sort();
    names.sort();
    assert_eq!(registered, names);
    assert_eq!(registered.len(), 12);
    for table in crm_tables() {
        let columns = db
            .conn()
            .prepare("SELECT name,pk FROM pragma_table_xinfo(?1) WHERE hidden=0 ORDER BY cid")
            .unwrap()
            .query_map([table.name()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<crate::sql::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            table.columns(),
            columns
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
        );
        let mut keys = columns
            .iter()
            .filter(|(_, position)| *position > 0)
            .collect::<Vec<_>>();
        keys.sort_by_key(|(_, position)| *position);
        assert_eq!(
            table.key_columns(),
            keys.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        crm_table("erased_corporate_numbers").unwrap().key_columns(),
        &["number_hash"]
    );
    assert!(
        !crm_table("companies")
            .unwrap()
            .columns()
            .contains(&"has_phone")
    );
    assert!(
        crm_table("counts")
            .unwrap()
            .columns()
            .contains(&"has_phone")
    );
}

#[test]
fn registry_never_admits_control_derived_or_arbitrary_table_names() {
    for name in [
        "users",
        "sessions",
        "remote_gate",
        "erase_gate",
        "name_fts",
        "name_fts_data",
        "sqlite_sequence",
        "COMPANIES",
        "companies;DROP TABLE companies",
    ] {
        assert!(crm_table(name).is_none());
    }
}

#[test]
fn fresh_source_sequence_is_explicit_zero_and_snapshot_absence_is_invalid() {
    let (_directory, db) = database();
    let state = read_crm_sequences(db.conn()).unwrap();
    assert_eq!(state.high_water(), 0);
    assert_eq!(
        encode_crm_sequences(state).unwrap(),
        br#"{"custom_field_definitions":"0"}"#
    );
    assert_eq!(
        decode_crm_sequences(br#"{"custom_field_definitions":"0"}"#, 0).unwrap(),
        state
    );
    assert_eq!(
        decode_crm_sequences(b"{}", 0),
        Err(SnapshotError::InvalidSequence)
    );
}

#[test]
fn deleted_largest_and_all_deleted_field_ids_keep_the_source_high_water() {
    let (_directory, db) = database();
    for id in 1..=3_i64 {
        db.conn()
            .execute(
                "INSERT INTO custom_field_definitions(id,name,kind,is_indexed) VALUES(?1,?2,1,0)",
                params![id, format!("field{id}")],
            )
            .unwrap();
    }
    db.conn()
        .execute("DELETE FROM custom_field_definitions WHERE id>1", [])
        .unwrap();
    assert_eq!(read_crm_sequences(db.conn()).unwrap().high_water(), 3);
    db.conn()
        .execute("DELETE FROM custom_field_definitions", [])
        .unwrap();
    assert_eq!(read_crm_sequences(db.conn()).unwrap().high_water(), 3);
}

#[test]
fn source_sequence_rejects_unknown_duplicate_missing_or_invalid_entries() {
    for mutation in [
        "INSERT INTO sqlite_sequence(name,seq) VALUES('unknown',5)",
        "INSERT INTO sqlite_sequence(name,seq) VALUES('custom_field_definitions',5)",
        "DELETE FROM sqlite_sequence",
        "UPDATE sqlite_sequence SET seq=-1",
        "UPDATE sqlite_sequence SET seq=4",
        "UPDATE sqlite_sequence SET seq='private-sequence-input'",
        "UPDATE sqlite_sequence SET seq=1.5",
    ] {
        let (_directory, db) = database();
        db.conn().execute("INSERT INTO custom_field_definitions(id,name,kind,is_indexed) VALUES(5,'field',1,0)",[]).unwrap();
        db.conn().execute_batch(mutation).unwrap();
        assert_eq!(
            read_crm_sequences(db.conn()),
            Err(SnapshotError::InvalidSequence)
        );
    }
}

#[test]
fn sequence_map_rejects_missing_unknown_duplicate_noncanonical_and_inconsistent_values() {
    for bytes in [
        "{}",
        r#"{"unknown":"0"}"#,
        r#"{"custom_field_definitions":"0","unknown":"0"}"#,
        r#"{"custom_field_definitions":"0","custom_field_definitions":"0"}"#,
        r#"{"custom_field_definitions":"-1"}"#,
        r#"{"custom_field_definitions":"01"}"#,
        r#"{"custom_field_definitions":"+1"}"#,
        r#"{"custom_field_definitions":"-0"}"#,
        r#"{"custom_field_definitions":"9223372036854775808"}"#,
        r#"{"custom_field_definitions":1}"#,
        r#"{ "custom_field_definitions":"0"}"#,
    ] {
        assert_eq!(
            decode_crm_sequences(bytes.as_bytes(), 0),
            Err(SnapshotError::InvalidSequence)
        );
    }
    assert_eq!(
        decode_crm_sequences(br#"{"custom_field_definitions":"4"}"#, 5),
        Err(SnapshotError::InvalidSequence)
    );
    assert_eq!(
        decode_crm_sequences(br#"{"custom_field_definitions":"0"}"#, -1),
        Err(SnapshotError::InvalidSequence)
    );
    let bytes = br#"{"custom_field_definitions":"9223372036854775807"}"#;
    let state = decode_crm_sequences(bytes, 0).unwrap();
    assert_eq!(state.high_water(), i64::MAX);
    assert_eq!(encode_crm_sequences(state).unwrap(), bytes);
}

#[test]
fn codec_and_sequence_diagnostics_never_echo_untrusted_input() {
    let marker = "private-snapshot-input";
    let row = format!("{{\"version\":1,\"values\":[{{\"type\":\"{marker}\"}}]}}");
    let error = decode_row(row.as_bytes(), 1).unwrap_err();
    assert!(!format!("{error} {error:?}").contains(marker));
    let sequence = format!("{{\"custom_field_definitions\":\"{marker}\"}}");
    let error = decode_crm_sequences(sequence.as_bytes(), 0).unwrap_err();
    assert!(!format!("{error} {error:?}").contains(marker));
}
