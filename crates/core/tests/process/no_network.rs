use std::{
    fs,
    path::{Path, PathBuf},
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
fn dependency_packages_and_deny_policy_exclude_network() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    let names: Vec<_> = lock
        .split("[[package]]")
        .skip(1)
        .map(|package| {
            package
                .lines()
                .find_map(|line| line.trim().strip_prefix("name = "))
                .unwrap()
                .trim_matches('"')
        })
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
