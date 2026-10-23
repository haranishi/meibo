//! Safe CSV export regression tests using synthetic companies only.
mod support;

use std::io::{self, Write};

use encoding_rs::SHIFT_JIS;
use meibo_import::{Encoding, ExportOptions, ImportOptions, export_csv, import_csv};
use support::{companies, company, database, verify};

// REQ 13.1, REQ 13.2: excluded and personal data each require their explicit switch.
#[test]
fn excludes_personal_excluded_and_erased_rows() {
    let (_directory, mut engine) = database();
    engine.register_company(&company("合成一般")).unwrap();
    let mut personal = company("個人秘密");
    personal.is_sole_proprietor = true;
    personal.phone = Some("personal-phone".into());
    personal.website = Some("https://private.invalid".into());
    engine.register_company(&personal).unwrap();
    let mut excluded = company("合成除外");
    excluded.status = meibo_core::types::Status::Excluded;
    engine.register_company(&excluded).unwrap();
    let mut erased_input = company("消去秘密");
    erased_input.is_sole_proprietor = true;
    let erased = engine.register_company(&erased_input).unwrap();
    engine.erase_company(erased).unwrap();
    for (include_personal, include_excluded, expected) in [
        (false, false, 1),
        (true, false, 2),
        (false, true, 2),
        (true, true, 3),
    ] {
        let mut output = Vec::new();
        assert_eq!(
            export_csv(
                &engine,
                &mut output,
                ExportOptions {
                    include_personal,
                    include_excluded,
                    ..ExportOptions::default()
                }
            )
            .unwrap(),
            expected
        );
        let csv = String::from_utf8(output).unwrap();
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("合成一般"));
        assert_eq!(csv.contains("個人秘密"), include_personal);
        assert_eq!(csv.contains("personal-phone"), include_personal);
        assert_eq!(csv.contains("private.invalid"), include_personal);
        assert_eq!(csv.contains("合成除外"), include_excluded);
        assert!(!csv.contains("消去秘密"));
        assert!(!csv.contains("contact_name"));
        assert!(!csv.contains("note"));
    }
    verify(&engine);
}

// REQ 13.3: every textual formula prefix is protected and stays protected on import.
#[test]
fn protects_formulas_preserves_quoting_and_roundtrips() {
    let (_directory, mut engine) = database();
    let names = [
        "=SUM(1,2)",
        " +cmd",
        "-1+2",
        "@cmd",
        "\tplain",
        "\rplain",
        "\nplain",
        "合成,\"引用\"\n第二",
    ];
    for name in names {
        let mut input = company(name);
        input.phone = Some("-03-1234".into());
        input.website = Some("@synthetic.invalid".into());
        input.source_id = Some(" =formula".into());
        engine.register_company(&input).unwrap();
    }
    let mut bytes = Vec::new();
    assert_eq!(
        export_csv(&engine, &mut bytes, ExportOptions::default()).unwrap(),
        8
    );
    let csv = std::str::from_utf8(&bytes).unwrap();
    assert!(csv.contains("\"'=SUM(1,2)\""));
    assert!(csv.contains("合成,\"\"引用\"\"\n第二"));
    assert!(csv.contains("\r\n"));
    let (_second_directory, mut second) = database();
    import_csv(&mut second, bytes.as_slice(), ImportOptions::default()).unwrap();
    let rows = companies(&second);
    for (index, name) in names.into_iter().enumerate() {
        let expected = if index < 7 {
            format!("'{name}")
        } else {
            name.to_owned()
        };
        assert_eq!(rows[index].name.as_deref(), Some(expected.as_str()));
        assert_eq!(rows[index].phone.as_deref(), Some("'-03-1234"));
        assert_eq!(rows[index].website.as_deref(), Some("'@synthetic.invalid"));
        assert_eq!(rows[index].source_id.as_deref(), Some("' =formula"));
    }
    verify(&second);
}

// REQ 11.1, REQ 13.3: Shift_JIS is strict on both sides and no replacement is written.
#[test]
fn strict_shift_jis_export_and_unrepresentable_character() {
    let (_directory, mut engine) = database();
    engine.register_company(&company("合成ソフト")).unwrap();
    let mut bytes = Vec::new();
    let options = ExportOptions {
        encoding: Encoding::ShiftJis,
        ..ExportOptions::default()
    };
    assert_eq!(export_csv(&engine, &mut bytes, options).unwrap(), 1);
    assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
    let decoded = SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(&bytes)
        .unwrap();
    assert!(decoded.contains("合成ソフト"));
    assert!(decoded.ends_with("\r\n"));
    let (_other_directory, mut other) = database();
    import_csv(
        &mut other,
        bytes.as_slice(),
        ImportOptions {
            encoding: Encoding::ShiftJis,
            ..ImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(companies(&other)[0].name.as_deref(), Some("合成ソフト"));
    engine.register_company(&company("合成🦀秘密")).unwrap();
    let error = export_csv(&engine, Vec::new(), options).unwrap_err();
    assert_eq!(error.row, 3);
    assert!(!format!("{error:?} {error}").contains("秘密"));
    assert!(error.to_string().contains("Shift_JIS"));
}

// REQ 13.3: errors from header, row and final flush are returned without raw messages.
#[test]
fn writer_failures_do_not_become_success() {
    struct Fails {
        remaining: usize,
        fail_flush: bool,
    }
    impl Write for Fails {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("private-writer-detail"));
            }
            let count = self.remaining.min(bytes.len());
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::other("private-flush-detail"))
            } else {
                Ok(())
            }
        }
    }
    let (_directory, mut engine) = database();
    engine.register_company(&company("合成")).unwrap();
    for (remaining, fail_flush) in [(0, false), (210, false), (usize::MAX, true)] {
        let error = export_csv(
            &engine,
            Fails {
                remaining,
                fail_flush,
            },
            ExportOptions::default(),
        )
        .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("private"));
    }
    // The failed callback released the read transaction; subsequent writes still work.
    engine.register_company(&company("合成二社目")).unwrap();
    verify(&engine);
}
