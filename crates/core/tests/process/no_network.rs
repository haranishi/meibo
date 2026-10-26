use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

fn explicitly_gated_remote_fixture(root: &Path, file: &Path, text: &str) -> bool {
    let fixture = root.join("crates/core/tests/remote_lifecycle.rs");
    if file.canonicalize().unwrap() != fixture.canonicalize().unwrap() {
        return false;
    }
    assert!(
        text.lines()
            .take(8)
            .any(|line| line.trim() == "#![cfg(feature = \"integration-tests\")]"),
        "remote lifecycle fixture must be absent without its explicit feature"
    );
    let manifest = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    let setting = |line: &str| line.split_whitespace().collect::<String>();
    let target = manifest
        .split("[[test]]")
        .skip(1)
        .find(|block| {
            block
                .lines()
                .any(|line| setting(line) == "name=\"remote_lifecycle\"")
        })
        .expect("remote lifecycle fixture must be an explicit Cargo test target");
    for required in [
        "path=\"tests/remote_lifecycle.rs\"",
        "required-features=[\"integration-tests\"]",
    ] {
        assert!(target.lines().any(|line| setting(line) == required));
    }
    let features = manifest.split("[features]").nth(1).unwrap();
    assert!(features.lines().any(|line| {
        setting(line) == "integration-tests=[\"cloud\",\"meibo-db/integration-tests\",\"dep:tokio\",\"dep:reqwest\",\"dep:libsql-hrana\"]"
    }));
    true
}

// REQ 7.4
#[test]
fn rust_sources_have_no_network_symbols() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let this_file = root
        .join("crates/core/tests/process/no_network.rs")
        .canonicalize()
        .unwrap();
    let mut files = Vec::new();
    // The local UI explicitly accepts loopback HTTP in app. The engine,
    // import library and development tools retain the no-network contract.
    for name in ["core", "import", "xtask"] {
        rust_files(&root.join("crates").join(name), &mut files);
    }
    assert!(files.len() > 1);
    for file in files {
        if file.canonicalize().unwrap() == this_file {
            continue;
        }
        let text = fs::read_to_string(&file).unwrap();
        // The one real-server acceptance fixture is not part of either default
        // artifacts or default test builds. Verify both permanent gates before
        // exempting its networking; production/default sources stay scanned.
        if explicitly_gated_remote_fixture(&root, &file, &text) {
            continue;
        }
        for symbol in [
            "std::net",
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "ToSocketAddrs",
        ] {
            assert!(
                !text.contains(symbol),
                "{} contains {symbol}",
                file.display()
            );
        }
    }
}

// REQ 7.4
#[test]
fn default_dependency_graph_and_deny_policy_exclude_network() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The lockfile also records the optional cloud adapter. Check the actual
    // default artifacts, retaining defaults so accidental cloud opt-in fails.
    let graph = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "--offline",
            "--package",
            "meibo-core",
            "--package",
            "meibo-app",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        graph.status.success(),
        "default dependency graph unavailable: {}",
        String::from_utf8_lossy(&graph.stderr)
    );
    let packages = String::from_utf8(graph.stdout).unwrap();
    let names: Vec<_> = packages
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(names.len() > 1);
    for forbidden in [
        "reqwest",
        "hyper",
        "ureq",
        "curl",
        "isahc",
        "surf",
        "attohttpc",
        "tokio",
        "async-std",
        "mio",
        "socket2",
        "h2",
        "openssl",
        "rustls",
        "native-tls",
        "hyper-util",
    ] {
        assert!(
            !names.contains(&forbidden),
            "network dependency: {forbidden}"
        );
    }
    let policy = fs::read_to_string(root.join("deny.toml")).unwrap();
    let bans = policy
        .split("[bans]")
        .nth(1)
        .unwrap()
        .split("[advisories]")
        .next()
        .unwrap();
    let deny = bans
        .split("deny = [")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    let banned: Vec<_> = deny
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            line.split_once("name = ")
                .map(|(_, tail)| tail.split('"').nth(1).unwrap())
        })
        .collect();
    for required in ["reqwest", "hyper", "ureq", "curl", "tokio", "openssl"] {
        assert!(banned.contains(&required), "deny policy missing {required}");
    }
}
