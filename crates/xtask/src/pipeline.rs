use crate::{command, docs_check, links, output, trace};
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const INTEGRATION_FEATURES: &str =
    "meibo-db/integration-tests,meibo-core/integration-tests,meibo-app/integration-tests";

fn execute(root: &Path, args: &[&str]) -> bool {
    command(root, "cargo", args)
        .status()
        .is_ok_and(|s| s.success())
}

fn require_checker(root: &Path) -> bool {
    if output(root, "cargo-deny", &["--version"]).is_ok_and(|o| o.status.success()) {
        true
    } else {
        eprintln!("cargo-deny is required; install the version in CONTRIBUTING.md");
        false
    }
}

fn require_server(root: &Path) -> bool {
    let binary = env::var_os("MEIBO_SQLD_BINARY").unwrap_or_else(|| {
        env::var_os("HOME")
            .map(|home| Path::new(&home).join(".turso/sqld"))
            .filter(|path| path.is_file())
            .map_or_else(|| OsString::from("sqld"), PathBuf::into_os_string)
    });
    if Command::new(binary)
        .arg("--version")
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout.starts_with(b"sqld "))
    {
        true
    } else {
        eprintln!("sqld is required; set MEIBO_SQLD_BINARY to the compatible fixture binary");
        false
    }
}

fn dependency_policy(root: &Path, cloud: bool) -> bool {
    let mut args = vec!["deny"];
    if cloud {
        args.extend([
            "--workspace",
            "--config",
            "deny-cloud.toml",
            "--all-features",
        ]);
    } else {
        args.extend([
            "--manifest-path",
            "crates/app/Cargo.toml",
            "--config",
            "deny.toml",
        ]);
    }
    args.extend(["--locked", "check", "all"]);
    execute(root, &args)
}

pub(super) fn check_all(root: &Path, _quick: bool) -> i32 {
    if !require_checker(root) {
        return 1;
    }
    let mut failed = Vec::new();
    for (name, args) in [
        ("fmt", vec!["fmt", "--all", "--", "--check"]),
        (
            "clippy",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ),
        ("test", vec!["test", "--workspace", "--locked"]),
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
    if !dependency_policy(root, false) {
        failed.push("deny");
    }
    println!("失敗: {failed:?}");
    i32::from(!failed.is_empty())
}

pub(super) fn check_cloud(root: &Path) -> i32 {
    if !require_checker(root) || !require_server(root) {
        return 1;
    }
    let mut failed = Vec::new();
    for (name, args) in [
        ("fmt", vec!["fmt", "--all", "--", "--check"]),
        (
            "cloud-build",
            vec![
                "build",
                "--workspace",
                "--features",
                "meibo-app/cloud",
                "--locked",
            ],
        ),
        (
            "cloud-clippy",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--features",
                INTEGRATION_FEATURES,
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ),
        (
            "cloud-test",
            vec![
                "test",
                "--workspace",
                "--all-targets",
                "--features",
                INTEGRATION_FEATURES,
                "--locked",
            ],
        ),
    ] {
        println!("== {name}");
        if !execute(root, &args) {
            failed.push(name);
        }
    }
    if !dependency_policy(root, true) {
        failed.push("cloud-deny");
    }
    println!("失敗: {failed:?}");
    i32::from(!failed.is_empty())
}
