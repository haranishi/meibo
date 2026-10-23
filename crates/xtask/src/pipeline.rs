use crate::{command, docs_check, links, output, trace};
use std::path::Path;
fn execute(root: &Path, args: &[&str]) -> bool {
    command(root, "cargo", args)
        .status()
        .is_ok_and(|s| s.success())
}
pub(super) fn check_all(root: &Path, _quick: bool) -> i32 {
    let mut failed = Vec::new();
    for (name, args) in [
        ("fmt", vec!["fmt", "--all", "--", "--check"]),
        (
            "clippy",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        ("test", vec!["test", "--workspace"]),
    ] {
        println!("== {name}");
        if !execute(root, &args) {
            failed.push(name);
        }
    }
    if !links::run(root).is_ok_and(|c| c == 0) {
        failed.push("links");
    }
    if !trace::run(root, false).is_ok_and(|c| c == 0) {
        failed.push("trace");
    }
    if !docs_check::run(root, false).is_ok_and(|c| c == 0) {
        failed.push("docs-check");
    }
    if output(root, "cargo-deny", &["--version"]).is_ok_and(|o| o.status.success()) {
        if !execute(root, &["deny", "check", "licenses", "bans", "sources"]) {
            failed.push("deny");
        }
    } else {
        println!("cargo-deny未導入: 未実行");
    }
    println!("失敗: {failed:?}");
    i32::from(!failed.is_empty())
}
