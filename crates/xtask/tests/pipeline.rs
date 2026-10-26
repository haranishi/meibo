//! The public validation commands must run their complete profiles and fail closed.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Harness {
    directory: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

impl Harness {
    fn new(checker: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join(".cargo/bin");
        fs::create_dir_all(&bin).unwrap();
        executable(
            &bin.join("cargo"),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$MEIBO_PIPELINE_LOG\"\nif [ \"$1\" = \"$MEIBO_PIPELINE_FAIL\" ]; then exit 42; fi\nexit 0\n",
        );
        executable(
            &bin.join("sqld"),
            "#!/bin/sh\nprintf 'sqld synthetic-validation-fixture\\n'\n",
        );
        if checker {
            executable(&bin.join("cargo-deny"), "#!/bin/sh\nexit 0\n");
        }
        let log = directory.path().join("commands.log");
        Self {
            directory,
            bin,
            log,
        }
    }

    fn command(&self, command: &str) -> Command {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"));
        child
            .arg(command)
            .env("HOME", self.directory.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("MEIBO_PIPELINE_LOG", &self.log)
            .env("MEIBO_PIPELINE_FAIL", "")
            .env("MEIBO_SQLD_BINARY", self.bin.join("sqld"));
        child
    }

    fn commands(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn diagnostic(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn default_validation_requires_the_policy_checker() {
    let fixture = Harness::new(false);
    let output = fixture.command("check-all").output().unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    assert!(diagnostic(&output).contains("cargo-deny"));
}

#[test]
fn default_validation_uses_the_actual_app_graph_and_propagates_policy_failure() {
    let fixture = Harness::new(true);
    let output = fixture
        .command("check-all")
        .env("MEIBO_PIPELINE_FAIL", "deny")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    let commands = fixture.commands();
    let policy = commands
        .lines()
        .find(|line| line.starts_with("deny "))
        .expect("the default policy must actually run");
    assert!(policy.contains("--manifest-path crates/app/Cargo.toml"));
    assert!(policy.contains("--config deny.toml"));
    assert!(policy.ends_with("check all"));
    for unsupported in ["--workspace", "--all-features", "--no-default-features"] {
        assert!(!policy.contains(unsupported), "{policy}");
    }
}

#[test]
fn cloud_validation_runs_production_build_and_all_integration_targets() {
    let fixture = Harness::new(true);
    let output = fixture
        .command("check-cloud")
        .env("MEIBO_PIPELINE_FAIL", "test")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    let commands = fixture.commands();
    let build = commands
        .lines()
        .find(|line| line.starts_with("build "))
        .expect("a production cloud build must actually run");
    assert!(build.contains("--workspace"));
    assert!(build.contains("--locked"));
    assert!(build.contains("--features meibo-app/cloud"));
    assert!(!build.contains("integration-tests"));
    for operation in ["clippy ", "test "] {
        let command = commands
            .lines()
            .find(|line| line.starts_with(operation))
            .expect("all integration targets must actually run");
        for required in [
            "--workspace",
            "--all-targets",
            "--locked",
            "meibo-db/integration-tests",
            "meibo-core/integration-tests",
            "meibo-app/integration-tests",
        ] {
            assert!(command.contains(required), "{command}");
        }
        for skip in ["--no-run", "--exclude", "--test ", "--no-default-features"] {
            assert!(!command.contains(skip), "{command}");
        }
    }
    let policy = commands
        .lines()
        .find(|line| line.starts_with("deny "))
        .expect("the cloud policy must actually run");
    for required in ["--workspace", "--all-features", "--config deny-cloud.toml"] {
        assert!(policy.contains(required), "{policy}");
    }
    assert!(policy.ends_with("check all"));
}

#[test]
fn cloud_validation_refuses_a_missing_server_before_cargo() {
    let fixture = Harness::new(true);
    let output = fixture
        .command("check-cloud")
        .env(
            "MEIBO_SQLD_BINARY",
            fixture.directory.path().join("missing-sqld"),
        )
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    assert!(diagnostic(&output).contains("sqld is required"));
    assert_eq!(fixture.commands(), "");
}

#[test]
fn sqld_setup_rejects_an_unverified_archive_without_installing() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("wrong.tar.xz");
    fs::write(&archive, b"not the trusted sqld release").unwrap();
    let destination = directory.path().join("tools");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("/bin/sh")
        .arg(root.join("scripts/install-ci-sqld.sh"))
        .arg(&destination)
        .arg(&archive)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    assert!(diagnostic(&output).contains("checksum mismatch"));
    assert!(!destination.join("sqld").exists());
}
