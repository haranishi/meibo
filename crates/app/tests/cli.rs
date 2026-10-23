//! Local CLI flows over synthetic company data and isolated files.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meibo"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let result = run(dir, args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn companies(dir: &Path, database: &str) -> Vec<serde_json::Value> {
    ok(dir, &["list", database, "--limit", "200"])
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

const SAMPLE: &str = "origin,name,prefecture,corporate_number,is_sole_proprietor,phone\r\n\
synthetic,株式会社合成テスト,13,6600000000000,false,０３１２３４５６７８\r\n\
synthetic,\"株式会社合成,改行\n見本\",27,,false,\r\n\
synthetic,個人の合成見本,1,,true,0000000000\r\n";

// REQ 14.1, REQ 14.2
#[test]
fn imports_searches_exports_reimports_and_verifies() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    fs::write(root.join("in.csv"), SAMPLE).unwrap();
    ok(root, &["init", "one.db", "--origin", "synthetic"]);
    assert!(ok(root, &["import", "one.db", "in.csv"]).contains("inserted=3 skipped=0"));
    let rows = companies(root, "one.db");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["phone"], "0312345678");
    assert!(rows[2]["name"].is_null());
    let found = ok(root, &["list", "one.db", "--search", "合成テスト"]);
    assert_eq!(found.lines().count(), 1);
    assert!(found.contains("株式会社合成テスト"));
    assert!(ok(root, &["export", "one.db", "out.csv"]).contains("exported=2"));
    let out = fs::read(root.join("out.csv")).unwrap();
    assert!(out.starts_with(&[0xef, 0xbb, 0xbf]));
    assert!(!String::from_utf8_lossy(&out).contains("個人の合成見本"));
    ok(root, &["init", "two.db", "--origin", "synthetic"]);
    assert!(ok(root, &["import", "two.db", "out.csv"]).contains("inserted=2"));
    assert_eq!(companies(root, "two.db").len(), 2);
    for db in ["one.db", "two.db"] {
        assert!(ok(root, &["verify", db]).contains("integrity=ok"));
    }
    assert!(
        ok(
            root,
            &["export", "one.db", "personal.csv", "--include-personal"]
        )
        .contains("exported=3")
    );
    assert!(
        fs::read_to_string(root.join("personal.csv"))
            .unwrap()
            .contains("個人の合成見本")
    );
}

// REQ 13.4, REQ 14.1
#[test]
fn existing_files_survive_and_failed_export_leaves_no_partial_file() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    fs::write(
        root.join("in.csv"),
        "origin,name,prefecture\nsynthetic,合成😀,13\n",
    )
    .unwrap();
    ok(root, &["init", "one.db", "--origin", "synthetic"]);
    ok(root, &["import", "one.db", "in.csv"]);
    assert!(
        !run(root, &["init", "one.db", "--origin", "synthetic"])
            .status
            .success()
    );
    assert!(!run(root, &["demo", "one.db"]).status.success());
    assert_eq!(companies(root, "one.db").len(), 1);
    fs::write(root.join("out.csv"), "keep this file").unwrap();
    assert!(!run(root, &["export", "one.db", "out.csv"]).status.success());
    assert_eq!(
        fs::read_to_string(root.join("out.csv")).unwrap(),
        "keep this file"
    );
    let failed = run(
        root,
        &[
            "export",
            "one.db",
            "unmappable.csv",
            "--encoding",
            "shift-jis",
        ],
    );
    assert!(!failed.status.success());
    assert!(!root.join("unmappable.csv").exists());
    assert!(!String::from_utf8_lossy(&failed.stderr).contains("合成😀"));
    for entry in fs::read_dir(root).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(!name.to_string_lossy().starts_with(".tmp"));
    }
    ok(root, &["verify", "one.db"]);
}

// REQ 12.3, REQ 12.4, REQ 14.2
#[test]
fn duplicate_error_and_explicit_skip_do_not_update_company() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    fs::write(
        root.join("in.csv"),
        "origin,name,prefecture,corporate_number\nsynthetic,元の合成企業,13,6600000000000\n",
    )
    .unwrap();
    ok(root, &["init", "one.db", "--origin", "synthetic"]);
    ok(root, &["import", "one.db", "in.csv"]);
    fs::write(root.join("in.csv"), "origin,name,prefecture,corporate_number\nsynthetic,上書きしない合成企業,27,6600000000000\n").unwrap();
    assert!(!run(root, &["import", "one.db", "in.csv"]).status.success());
    assert!(
        ok(
            root,
            &["import", "one.db", "in.csv", "--duplicates", "skip"]
        )
        .contains("inserted=0 skipped=1")
    );
    let rows = companies(root, "one.db");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "元の合成企業");
    assert_eq!(rows[0]["prefecture"], 13);
}

// REQ 14.1
#[test]
fn invalid_cli_is_rejected_before_creating_files() {
    let d = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "bad.db"],
        vec!["init", "bad.db", "--origin", "wrong"],
        vec!["demo", "bad.db", "--rows", "0"],
        vec!["demo", "bad.db", "--rows", "100001"],
        vec!["list", "bad.db", "--limit", "201"],
        vec!["import", "bad.db", "in.csv", "--encoding", "auto"],
        vec!["export", "bad.db", "out.csv", "--unknown"],
        vec!["demo", "bad.db", "--rows", "2", "--rows", "3"],
        vec!["verify", "bad.db", "extra"],
    ] {
        assert!(!run(d.path(), &args).status.success(), "{args:?}");
    }
    assert!(fs::read_dir(d.path()).unwrap().next().is_none());
    assert!(ok(d.path(), &["--help"]).contains("Earlier batches remain"));
    assert!(ok(d.path(), &["demo", "demo.db", "--rows", "12"]).contains("synthetic_companies=12"));
    ok(d.path(), &["verify", "demo.db"]);
}

// REQ 13.4
#[cfg(unix)]
#[test]
fn export_refuses_symlinks_and_uses_private_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    ok(root, &["demo", "one.db", "--rows", "2"]);
    symlink("missing", root.join("out.csv")).unwrap();
    assert!(!run(root, &["export", "one.db", "out.csv"]).status.success());
    assert_eq!(
        fs::read_link(root.join("out.csv")).unwrap(),
        Path::new("missing")
    );
    ok(root, &["export", "one.db", "private.csv"]);
    assert_eq!(
        fs::metadata(root.join("private.csv"))
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
}
