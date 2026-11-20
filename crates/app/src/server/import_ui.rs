//! Bounded company registration and contact supplementation for the current workspace.
use meibo_core::{DuplicatePolicy, Engine};
use meibo_import::{Encoding, ImportOptions, import_csv, supplement_contacts};
use serde_json::json;

use super::Reply;

pub(super) const MAX_IMPORT_BODY: usize = 32 * 1024 * 1024;

pub(super) fn import_csv_request(engine: &mut Engine, query: &str, body: &[u8]) -> Reply {
    if body.len() > MAX_IMPORT_BODY {
        return Reply::error(413, "CSVファイルは32MiB以下にしてください。");
    }
    let Some(options) = parse_options(query) else {
        return Reply::error(
            400,
            "取込方法、文字コード、重複時の処理を確認してください。",
        );
    };
    let options = match options {
        CsvAction::Register(options) => options,
        CsvAction::Supplement(encoding) => {
            return match supplement_contacts(engine, body, encoding) {
                Ok(report) => Reply::json(
                    200,
                    &json!({"ok":true,"updated":report.updated,
                "skipped":report.skipped,"partial_committed":false,"row":null}),
                ),
                Err(error) => Reply::json(
                    422,
                    &json!({"ok":false,"error":error.to_string(),
                "row":error.row,"updated":error.report.updated,"skipped":error.report.skipped,
                "partial_committed":error.report.updated > 0}),
                ),
            };
        }
    };
    match import_csv(engine, body, options) {
        Ok(report) => Reply::json(
            200,
            &json!({"ok":true,"inserted":report.inserted,"skipped":report.skipped,
                "partial_committed":false,"row":null}),
        ),
        Err(error) => Reply::json(
            422,
            &json!({"ok":false,"error":error.to_string(),"row":error.row,
                "inserted":error.report.inserted,"skipped":error.report.skipped,
                "partial_committed":error.report.inserted > 0 || error.report.skipped > 0}),
        ),
    }
}

enum CsvAction {
    Register(ImportOptions),
    Supplement(Encoding),
}

fn parse_options(query: &str) -> Option<CsvAction> {
    let mut options = ImportOptions::default();
    let mut encoding = false;
    let mut duplicate = false;
    let mut mode = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=')?;
        match (key, value) {
            ("mode", "supplement" | "register") if mode.is_none() => mode = Some(value),
            ("encoding", "utf8") if !encoding => {
                options.encoding = Encoding::Utf8;
                encoding = true;
            }
            ("encoding", "shift-jis") if !encoding => {
                options.encoding = Encoding::ShiftJis;
                encoding = true;
            }
            ("duplicate", "error") if !duplicate => {
                options.duplicates = DuplicatePolicy::Error;
                duplicate = true;
            }
            ("duplicate", "skip") if !duplicate => {
                options.duplicates = DuplicatePolicy::Skip;
                duplicate = true;
            }
            _ => return None,
        }
    }
    match (mode, duplicate) {
        (Some("supplement"), true) => None,
        (Some("supplement"), false) => Some(CsvAction::Supplement(options.encoding)),
        _ => Some(CsvAction::Register(options)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meibo_core::{
        OpenOptions,
        types::{DataOrigin, Filters},
    };
    use serde_json::Value;
    use std::fmt::Write as _;

    fn fixture() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::create(
            &dir.path().join("csv-demo.db"),
            DataOrigin::Synthetic,
            &OpenOptions::default(),
        )
        .unwrap();
        (dir, engine)
    }

    fn result(reply: &Reply) -> Value {
        serde_json::from_slice(&reply.body).unwrap()
    }

    // CRM essentials 7, 10: strict byte decoding, duplicate handling and synthetic origin.
    #[test]
    fn import_preserves_encoding_duplicate_and_origin_contracts() {
        let (_dir, mut engine) = fixture();
        let csv = b"origin,name,prefecture,corporate_number\nsynthetic,Example,13,6600000000000\n";
        let reply = import_csv_request(&mut engine, "encoding=utf8&duplicate=error", csv);
        assert_eq!(reply.status, 200);
        assert_eq!(result(&reply)["inserted"], 1);
        let duplicate = import_csv_request(&mut engine, "duplicate=error", csv);
        assert_eq!(duplicate.status, 422);
        assert_eq!(result(&duplicate)["inserted"], 0);
        assert_eq!(result(&duplicate)["partial_committed"], false);
        let skipped = import_csv_request(&mut engine, "duplicate=skip", csv);
        assert_eq!(skipped.status, 200);
        assert_eq!(result(&skipped)["skipped"], 1);

        // Japanese "合成" encoded as Windows-31J, sent without browser decoding.
        let sjis = b"origin,name,prefecture\nsynthetic,\x8d\x87\x90\xac,13\n";
        assert_eq!(
            import_csv_request(&mut engine, "encoding=utf8", sjis).status,
            422
        );
        assert_eq!(
            import_csv_request(&mut engine, "encoding=shift-jis", sjis).status,
            200
        );
        let real = b"origin,name,prefecture\nreal,Example,13\n";
        assert_eq!(import_csv_request(&mut engine, "", real).status, 422);
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 2);
    }

    // CRM essentials 7: a later bad record must report the first committed 500 rows.
    #[test]
    fn partial_failure_reports_only_committed_batches() {
        let (_dir, mut engine) = fixture();
        let mut csv = String::from("origin,name,prefecture\n");
        for index in 0..500 {
            writeln!(csv, "synthetic,Example {index},13").unwrap();
        }
        csv.push_str("synthetic,Uncommitted,13\nsynthetic,Invalid,99\n");
        let reply = import_csv_request(&mut engine, "", csv.as_bytes());
        assert_eq!(reply.status, 422);
        let data = result(&reply);
        assert_eq!(data["inserted"], 500);
        assert_eq!(data["skipped"], 0);
        assert_eq!(data["row"], 503);
        assert_eq!(data["partial_committed"], true);
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 500);
    }

    #[test]
    fn contact_supplement_uses_the_existing_workspace_and_can_be_replayed() {
        let (_dir, mut engine) = fixture();
        let company =
            b"origin,name,prefecture,corporate_number\nsynthetic,Example,05,6600000000000\n";
        assert_eq!(import_csv_request(&mut engine, "", company).status, 200);
        let csv = b"source,corporate_number,phone,website,industry\nsynthetic,6600000000000,03-0000-0000,,F\n";
        let query = "mode=supplement&encoding=utf8";
        let reply = import_csv_request(&mut engine, query, csv);
        assert_eq!(
            (reply.status, result(&reply)["updated"].as_u64()),
            (200, Some(1))
        );
        assert_eq!(
            result(&import_csv_request(&mut engine, query, csv))["skipped"],
            1
        );
        let wrong = b"source,corporate_number,phone,website,industry\nfirst_party,6600000000000,03-0000-0000,,F\n";
        let rejected = import_csv_request(&mut engine, query, wrong);
        assert_eq!(
            (rejected.status, result(&rejected)["row"].as_u64()),
            (422, Some(2))
        );
        assert_eq!(
            engine
                .count(&Filters {
                    has_phone: Some(true),
                    ..Filters::default()
                })
                .unwrap()
                .n,
            1
        );
    }

    // CRM essentials 10: validate the whole-file limit and reject ambiguous options before writing.
    #[test]
    fn oversized_csv_and_unknown_or_repeated_options_are_rejected() {
        let (_dir, mut engine) = fixture();
        for query in [
            "encoding=utf8&encoding=utf8",
            "duplicate=skip&duplicate=error",
            "encoding=auto",
            "other=value",
            "mode=supplement&duplicate=skip",
            "mode=other",
            "mode=supplement&mode=register",
        ] {
            assert_eq!(import_csv_request(&mut engine, query, b"").status, 400);
        }
        assert_eq!(
            import_csv_request(&mut engine, "", &vec![b'x'; MAX_IMPORT_BODY + 1]).status,
            413
        );
        assert_eq!(engine.count(&Filters::default()).unwrap().n, 0);
    }
}
