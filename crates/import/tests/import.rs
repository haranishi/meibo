//! Strict CSV import regression tests using synthetic companies only.
mod support;

use std::{
    fmt::Write as _,
    io::{self, Read},
};

use encoding_rs::SHIFT_JIS;
use meibo_core::{DuplicatePolicy, types::*};
use meibo_import::{Encoding, ImportOptions, ImportReport, import_csv};
use support::{companies, database, verify};

const HEADER: &str = "origin,name,prefecture\n";

// REQ 11.1, REQ 11.2, REQ 12.1: BOM, reordered fields, grammar and core normalization.
#[test]
fn reordered_bom_headers_quoted_newlines_and_all_fields() {
    let (_directory, mut engine) = database();
    let csv = concat!(
        "\u{feff}\"name\",prefecture,origin,corporate_number,municipality,industry,phone,website,owner,status,is_sole_proprietor,source_id,source_as_of\r\n",
        "\"合成,\"\"商事\"\"\r\n第二\",13,synthetic,6600000000000,13101,20,０３-１２３４, https://synthetic.invalid ,7,2,true,fixture,2024-02-29\r",
        "最後,1,synthetic,,,,,,,,0,,\n",
    );
    assert_eq!(
        import_csv(&mut engine, csv.as_bytes(), ImportOptions::default()).unwrap(),
        ImportReport {
            inserted: 2,
            skipped: 0
        }
    );
    let rows = companies(&engine);
    assert_eq!(rows[0].name.as_deref(), Some("合成,\"商事\"\r\n第二"));
    assert_eq!(rows[0].phone.as_deref(), Some("03-1234"));
    assert_eq!(
        rows[0].website.as_deref(),
        Some("https://synthetic.invalid")
    );
    assert_eq!(rows[0].corporate_number.as_deref(), Some("6600000000000"));
    assert_eq!(rows[0].municipality.as_deref(), Some("13101"));
    assert_eq!(rows[0].industry, Some(Industry::T));
    assert_eq!(rows[0].owner, Some(UserId(7)));
    assert_eq!(rows[0].status, Status::Negotiating);
    assert!(rows[0].is_sole_proprietor);
    assert_eq!(rows[0].source_id.as_deref(), Some("fixture"));
    assert_eq!(rows[0].source_as_of.as_deref(), Some("2024-02-29"));
    assert_eq!(rows[1].name.as_deref(), Some("最後"));
    verify(&engine);
}

// REQ 11.1: decoding is explicit, strict, and survives arbitrary reader boundaries.
#[test]
fn shift_jis_and_utf8_decode_without_replacement() {
    struct OneByte<'a>(&'a [u8]);
    impl Read for OneByte<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() || bytes.is_empty() {
                return Ok(0);
            }
            bytes[0] = self.0[0];
            self.0 = &self.0[1..];
            Ok(1)
        }
    }
    let (_directory, mut engine) = database();
    let (bytes, _, replacements) =
        SHIFT_JIS.encode("origin,name,prefecture\r\nsynthetic,合成ソフト,13");
    assert!(!replacements);
    import_csv(
        &mut engine,
        OneByte(&bytes),
        ImportOptions {
            encoding: Encoding::ShiftJis,
            ..ImportOptions::default()
        },
    )
    .unwrap();
    import_csv(
        &mut engine,
        OneByte("\u{feff}origin,name,prefecture\nsynthetic,合成株式会社,13".as_bytes()),
        ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(companies(&engine).len(), 2);
    for encoding in [Encoding::Utf8, Encoding::ShiftJis] {
        let mut invalid = HEADER.as_bytes().to_vec();
        invalid.extend_from_slice(b"synthetic,\x81,13\n");
        let error = import_csv(
            &mut engine,
            invalid.as_slice(),
            ImportOptions {
                encoding,
                ..ImportOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.row, 2);
        assert_eq!(error.report, ImportReport::default());
    }
    assert_eq!(companies(&engine).len(), 2);
}

// REQ 11.2, REQ 12.1: malformed grammar and schemas never save pending records.
#[test]
fn rejects_quotes_headers_types_and_core_validation() {
    let (_directory, mut engine) = database();
    let fixtures = [
        "",
        "name,prefecture\n合成,13",
        "origin,name,name,prefecture\n",
        "origin,name,prefecture,unexpected\n",
        "origin,name,prefecture\nsynthetic,合\"成,13",
        "origin,name,prefecture\nsynthetic,\"合成\" ,13",
        "origin,name,prefecture\nsynthetic,\"合成,13",
        "origin,name,prefecture\nsynthetic,合成",
        "origin,name,prefecture\nsynthetic,合成,13,extra",
        "origin,name,prefecture\n,合成,13",
        "origin,name,prefecture\nsynthetic,合成,48",
        "origin,name,prefecture\nsynthetic,合成,十三",
        "origin,name,prefecture\nsynthetic, ,13",
        "origin,name,prefecture\nreal,合成,13",
        "origin,name,prefecture,status\nsynthetic,合成,13,6",
        "origin,name,prefecture,industry\nsynthetic,合成,13,21",
        "origin,name,prefecture,is_sole_proprietor\nsynthetic,合成,13,yes",
        "origin,name,prefecture,source_as_of\nsynthetic,合成,13,2023-02-29",
        "origin,name,prefecture,municipality\nsynthetic,合成,13,01001",
        "origin,name,prefecture,corporate_number\nsynthetic,合成,13,1234567890123",
        "origin,name,prefecture,owner\nsynthetic,合成,13,invalid",
    ];
    for csv in fixtures {
        assert!(
            import_csv(&mut engine, csv.as_bytes(), ImportOptions::default()).is_err(),
            "{csv}"
        );
        assert_eq!(companies(&engine), []);
    }
    verify(&engine);
}

// REQ 11.2, REQ 11.3: bound raw records and columns before unbounded allocation.
#[test]
fn bounds_record_bytes_and_columns_without_echoing_values() {
    let (_directory, mut engine) = database();
    for bad in [
        format!("{HEADER}synthetic,{},13", "secret".repeat(175_000)),
        format!("{HEADER}synthetic,a,13,1,2,3,4,5,6,7,8,9,10,11"),
    ] {
        let error = import_csv(&mut engine, bad.as_bytes(), ImportOptions::default()).unwrap_err();
        assert_eq!(error.row, 2);
        assert!(!format!("{error:?} {error}").contains("secret"));
        assert_eq!(companies(&engine), []);
    }
}

// REQ 12.2, REQ 12.4: failures preserve only completed 500-record batches.
#[test]
fn syntax_conversion_and_io_failures_drop_pending_batch() {
    struct FailingReader {
        bytes: io::Cursor<Vec<u8>>,
    }
    impl Read for FailingReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = self.bytes.read(bytes)?;
            if count == 0 {
                Err(io::Error::other("private-input-must-not-be-printed"))
            } else {
                Ok(count)
            }
        }
    }
    for ending in [
        "synthetic,broken\"name,13",
        "synthetic,合成,not-a-number",
        "synthetic,合成,48",
    ] {
        let (_directory, mut engine) = database();
        let mut csv = String::from(HEADER);
        for index in 0..501 {
            writeln!(csv, "synthetic,合成{index},13").unwrap();
        }
        csv.push_str(ending);
        let error = import_csv(&mut engine, csv.as_bytes(), ImportOptions::default()).unwrap_err();
        assert_eq!(error.row, 503);
        assert_eq!(
            error.report,
            ImportReport {
                inserted: 500,
                skipped: 0
            }
        );
        assert_eq!(companies(&engine).len(), 500);
        verify(&engine);
    }
    let (_directory, mut engine) = database();
    let reader = FailingReader {
        bytes: io::Cursor::new(format!("{HEADER}synthetic,合成,13\n").into_bytes()),
    };
    let error = import_csv(&mut engine, reader, ImportOptions::default()).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("private-input"));
    assert_eq!(error.report, ImportReport::default());
    assert_eq!(companies(&engine), []);
}

// REQ 12.3: duplicates roll back their batch or skip without updating existing values.
#[test]
fn duplicate_corporate_numbers_and_unnumbered_names() {
    let (_directory, mut engine) = database();
    let header = "origin,name,prefecture,corporate_number\n";
    let duplicate =
        format!("{header}synthetic,合成元,13,6600000000000\nsynthetic,合成変更,13,6600000000000\n");
    let error =
        import_csv(&mut engine, duplicate.as_bytes(), ImportOptions::default()).unwrap_err();
    assert_eq!(error.row, 3);
    assert_eq!(companies(&engine), []);
    let options = ImportOptions {
        duplicates: DuplicatePolicy::Skip,
        ..ImportOptions::default()
    };
    assert_eq!(
        import_csv(&mut engine, duplicate.as_bytes(), options).unwrap(),
        ImportReport {
            inserted: 1,
            skipped: 1
        }
    );
    assert_eq!(companies(&engine)[0].name.as_deref(), Some("合成元"));
    assert_eq!(
        import_csv(&mut engine, duplicate.as_bytes(), options).unwrap(),
        ImportReport {
            inserted: 0,
            skipped: 2
        }
    );
    import_csv(
        &mut engine,
        format!("{HEADER}synthetic,合成同名,13\nsynthetic,合成同名,13\n").as_bytes(),
        ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(companies(&engine).len(), 3);
    verify(&engine);
}

// REQ 12.2, REQ 12.4: byte-bounded batches commit independently of the 500-row limit.
#[test]
fn eight_mib_string_limit_commits_before_next_batch_failure() {
    let (_directory, mut engine) = database();
    let header = "origin,name,prefecture,website\n";
    let row = format!(
        "synthetic,合成,13,https://synthetic.invalid/{}\n",
        "a".repeat(900_000)
    );
    let input = format!("{header}{}synthetic,broken\"name,13,\n", row.repeat(10));
    let error = import_csv(&mut engine, input.as_bytes(), ImportOptions::default()).unwrap_err();
    assert_eq!(error.row, 12);
    assert_eq!(
        error.report,
        ImportReport {
            inserted: 9,
            skipped: 0
        }
    );
    assert_eq!(companies(&engine).len(), 9);
    verify(&engine);
}

// REQ 12.3, REQ 12.4: skipped counts also exclude uncommitted batches.
#[test]
fn only_committed_skips_are_reported_on_failure() {
    let (_directory, mut engine) = database();
    let row = "synthetic,合成,13,6600000000000\n";
    let input = format!(
        "origin,name,prefecture,corporate_number\n{}broken",
        row.repeat(501)
    );
    let error = import_csv(
        &mut engine,
        input.as_bytes(),
        ImportOptions {
            duplicates: DuplicatePolicy::Skip,
            ..ImportOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.row, 503);
    assert_eq!(
        error.report,
        ImportReport {
            inserted: 1,
            skipped: 499
        }
    );
    assert_eq!(companies(&engine).len(), 1);
    verify(&engine);
}
