use crate::{Result, command, lines};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const EXCLUDES: &[&str] = &[
    ".agent-harness",
    ".kiro",
    "target",
    ".git",
    "node_modules",
    ".cargo-home",
    ".cargo-target",
    "screens-out",
    "test-results",
    "playwright-report",
    "e2e",
    "ui",
];
#[derive(Deserialize)]
struct Spec {
    test_cmd: Vec<String>,
    timeout_sec: Option<u64>,
    exclude: Option<Vec<String>>,
    mutations: Vec<Mutation>,
}
#[derive(Deserialize)]
struct Mutation {
    label: String,
    file: String,
    find: String,
    replace: String,
    count: Option<usize>,
}
struct Temporary {
    path: PathBuf,
    keep: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn temporary(temp_dir: &Path, keep: bool) -> Result<Temporary> {
    fs::create_dir_all(temp_dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = temp_dir.join(format!("meibo-mut-{}-{stamp}", std::process::id()));
    fs::create_dir(&path)?;
    Ok(Temporary { path, keep })
}
fn copy_tree(src: &Path, dst: &Path, excludes: &[&str], base: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path == base || excludes.iter().any(|n| entry.file_name() == *n) {
            continue;
        }
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_symlink() {
            copy_link(&path, &target)?;
        } else if path.is_dir() {
            copy_tree(&path, &target, excludes, base)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    fs::set_permissions(dst, fs::metadata(src)?.permissions())?;
    Ok(())
}
#[cfg(unix)]
fn copy_link(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
    Ok(())
}
#[cfg(not(unix))]
fn copy_link(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst)?;
    Ok(())
}
fn classify(code: i32, text: &str) -> (i32, &'static str, String) {
    if code == 0 {
        return (0, "ok", String::new());
    }
    if code < 0 {
        return (code, "signal", String::new());
    }
    if !matches!(code, 1 | 101) {
        return (code, "process", String::new());
    }
    if text.contains("could not compile") {
        return (code, "compile", String::new());
    }
    let mut names = Vec::new();
    for line in lines(text) {
        if let Some(name) = line
            .trim()
            .strip_prefix("test ")
            .and_then(|s| s.strip_suffix(" ... FAILED"))
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
        {
            names.push(name.to_owned());
        }
    }
    let mut in_list = false;
    for line in lines(text) {
        if line.trim() == "failures:" {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(name) = line.strip_prefix("    ").filter(|s| {
                let mut chars = s.chars();
                chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || "_:".contains(c))
            }) {
                names.push(name.to_owned());
            } else if line.trim().is_empty() && !names.is_empty() {
                in_list = false;
            }
        }
    }
    let mut unique = Vec::new();
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    (
        code,
        if unique.is_empty() { "process" } else { "test" },
        unique.into_iter().take(4).collect::<Vec<_>>().join(", "),
    )
}
fn run_tests(
    work: &Path,
    base: &Path,
    cmd: &[String],
    timeout: u64,
) -> Result<(i32, &'static str, String)> {
    let (program, args) = cmd.split_first().ok_or("test_cmd が空です")?;
    // ファイルへ出力を受け、子プロセス実行中のパイプ詰まりとメモリの増大を防ぐ。
    let stdout = base.join("stdout");
    let stderr = base.join("stderr");
    let mut child = command(
        work,
        program,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .env("CARGO_TARGET_DIR", base.join("target"))
    .stdout(Stdio::from(fs::File::create(&stdout)?))
    .stderr(Stdio::from(fs::File::create(&stderr)?))
    .spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(timeout) {
            child.kill()?;
            child.wait()?;
            return Ok((124, "timeout", "時間切れ".to_owned()));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut text = String::from_utf8_lossy(&fs::read(stdout)?).into_owned();
    text.push_str(&String::from_utf8_lossy(&fs::read(stderr)?));
    Ok(classify(exit_code(status), &text))
}
fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().and_then(i32::checked_neg))
            .unwrap_or(-1)
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(-1)
    }
}
fn source_text(path: &Path) -> Result<String> {
    // 改行の違い(CRLF・CR)で結果が変わらないよう、個数の確認と置換の前にLFへそろえる。
    Ok(fs::read_to_string(path)?
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

fn apply(text: &str, m: &Mutation) -> std::result::Result<String, usize> {
    let count = if m.find.is_empty() {
        text.chars().count().saturating_add(1)
    } else {
        text.matches(&m.find).count()
    };
    if count != m.count.unwrap_or(1) {
        return Err(count);
    }
    Ok(text.replace(&m.find, &m.replace))
}
fn options(args: &[String]) -> Result<(&str, Option<&str>, bool)> {
    let path = args
        .first()
        .ok_or("使い方: cargo xtask mutate <定義のJSON> [--only 文字列] [--keep]")?;
    let only = args
        .iter()
        .position(|s| s == "--only")
        .map(|i| {
            args.get(i.saturating_add(1))
                .map(String::as_str)
                .ok_or("--only に文字列が必要です")
        })
        .transpose()?;
    Ok((path, only, args.iter().any(|a| a == "--keep")))
}
const HELP: &str = "変異試験の道具: わざと誤りを入れて、試験が赤くなるかを確かめる。\n\n使い方:  cargo xtask mutate crates/xtask/mutations.json [--only 文字列] [--keep]\n\n仕様のJSON:\n  {\n    \"test_cmd\": [\"cargo\", \"test\", \"-q\", \"-p\", \"meibo-core\", \"--no-fail-fast\"],\n    \"timeout_sec\": 900,\n    \"mutations\": [\n      {\"label\": \"按分: 四捨五入を切捨てに\", \"file\": \"crates/core/src/houjin.rs\",\n       \"find\": \"+ 50) / 100\", \"replace\": \"/ 100\"}\n    ]\n  }\n\nリポジトリ全体をコピーした作業用の場所で試すので、本物のファイルは変わらない。\nファイルを戻すときは更新時刻を引き継がない(コピーして新しい時刻にする)。引き継ぐと、Cargoが「変更なし」と\n判断して前の変異を含むビルドを使い回し、結果が嘘になる。生き残り(試験が赤くならない変異)が\nあれば終了コード1。基準(変異なし)が赤い場合は、変異の結果を信用できないので終了コード2。\nコンパイルエラーは「検出」に数えず、直すべき変異として一覧に出す。\n";

/// 変異させるのは `crates/core/src` の本体のソースだけ。試験と道具は対象にしない。
fn validate_targets(selected: &[&Mutation]) -> Result<()> {
    for mutation in selected {
        let path = Path::new(&mutation.file);
        if !path.starts_with("crates/core/src")
            || path.extension().is_none_or(|e| e != "rs")
            || path.components().any(|c| c.as_os_str() == "..")
            || path.file_name().is_some_and(|n| n == "tests.rs")
        {
            return Err(format!(
                "変異の対象は crates/core/src の本体のソースだけです: {}",
                mutation.file
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn run(root: &Path, args: &[String], temp_dir: &Path) -> Result<i32> {
    if args.is_empty() {
        println!("{HELP}");
        return Ok(2);
    }
    let (path, only, keep) = options(args)?;
    let spec: Spec = serde_json::from_str(&fs::read_to_string(path)?)?;
    let selected: Vec<_> = spec
        .mutations
        .iter()
        .filter(|mutation| only.is_none_or(|s| mutation.label.contains(s)))
        .collect();
    if selected.is_empty() {
        println!("適用する変異がありません。定義と --only を確認してください。");
        return Ok(2);
    }
    validate_targets(&selected)?;
    let started = Instant::now();
    let base = temporary(temp_dir, keep)?;
    let work = base.path.join("repo");
    let excludes = spec.exclude.as_ref().map_or_else(
        || EXCLUDES.to_vec(),
        |v| v.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    copy_tree(root, &work, &excludes, &base.path)?;
    let timeout = spec.timeout_sec.unwrap_or(900);
    let (code, _, _) = run_tests(&work, &base.path, &spec.test_cmd, timeout)?;
    println!("[基準] 変更なし: 終了コード={code}（0であること）");
    if code != 0 {
        println!("基準が赤いので、変異の結果は信用できません。");
        return Ok(2);
    }
    let mut survivors = Vec::new();
    for mutation in selected {
        if started.elapsed() >= Duration::from_secs(3600) {
            println!("1時間の上限で停止。残りは未実行。");
            return Ok(2);
        }
        let target = work.join(&mutation.file);
        let original = root.join(&mutation.file);
        let text = source_text(&original)?;
        let changed = match apply(&text, mutation) {
            Ok(changed) => changed,
            Err(count) => {
                println!(
                    "[{}] 適用できない: 見つかった数={count}, 期待={}",
                    mutation.label,
                    mutation.count.unwrap_or(1)
                );
                survivors.push(format!("{}（適用できない）", mutation.label));
                continue;
            }
        };
        fs::write(&target, changed)?;
        thread::sleep(Duration::from_millis(1100));
        let result = run_tests(&work, &base.path, &spec.test_cmd, timeout);
        fs::copy(original, target)?;
        thread::sleep(Duration::from_millis(1100));
        let (_, kind, who) = result?;
        match kind {
            "ok" => {
                println!("[{}] 生き残り ← 試験が赤くならなかった", mutation.label);
                survivors.push(mutation.label.clone());
            }
            "compile" => {
                println!(
                    "[{}] 変異がコンパイルできない ← 検出に数えない。変異の書き方を直すこと",
                    mutation.label
                );
                survivors.push(format!("{}（コンパイルできない）", mutation.label));
            }
            "test" => println!(
                "[{}] 検出 ← 赤になった試験: {}",
                mutation.label,
                if who.is_empty() {
                    "(名前を取れなかった)"
                } else {
                    &who
                }
            ),
            _ => {
                println!("[{}] 判定不能（{kind}） ← 検出に数えない", mutation.label);
                survivors.push(format!("{}（判定不能: {kind}）", mutation.label));
            }
        }
    }
    println!("\n== 生き残り ==");
    println!(
        "{}",
        if survivors.is_empty() {
            "なし".to_owned()
        } else {
            survivors.join("\n")
        }
    );
    Ok(i32::from(!survivors.is_empty()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failures() {
        assert_eq!(
            classify(1, "test x::y ... FAILED\nfailures:\n    x::y\n    z\n\n").2,
            "x::y, z"
        );
        assert_eq!(classify(1, "could not compile").1, "compile");
        assert_eq!(classify(0, "test x ... FAILED").1, "ok");
        assert_eq!(classify(-15, "test x ... FAILED").1, "signal");
        assert_eq!(classify(124, "test x ... FAILED").1, "process");
        assert_eq!(classify(1, "runner unavailable").1, "process");
        assert_eq!(classify(101, "test x ... FAILED").1, "test");
    }
    #[test]
    fn replacement_and_args() {
        let m = Mutation {
            label: String::new(),
            file: String::new(),
            find: "a".into(),
            replace: "b".into(),
            count: None,
        };
        assert_eq!(apply("a", &m).unwrap(), "b");
        assert_eq!(apply("aa", &m), Err(2));
        assert!(options(&[]).is_err());
        assert!(options(&["x".into(), "--only".into()]).is_err());
    }
    #[test]
    fn targets_are_core_sources_only() {
        let target = |file: &str| Mutation {
            label: String::new(),
            file: file.into(),
            find: "a".into(),
            replace: "b".into(),
            count: None,
        };
        let ok = [
            target("crates/core/src/write.rs"),
            target("crates/core/src/schema/counts.rs"),
        ];
        assert!(validate_targets(&ok.iter().collect::<Vec<_>>()).is_ok());
        for file in [
            "crates/xtask/src/lib.rs",
            "crates/core/tests/pure.rs",
            "crates/core/src/write/tests.rs",
            "crates/core/src/../Cargo.toml",
            "crates/core/src/../../xtask/src/lib.rs",
            "../outside.rs",
            "crates/core/src/schema/ddl.sql",
        ] {
            let bad = [target(file)];
            assert!(
                validate_targets(&bad.iter().collect::<Vec<_>>()).is_err(),
                "{file}"
            );
        }
    }
}
