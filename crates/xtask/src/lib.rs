//! リポジトリの開発用点検(`cargo xtask`)。リンク検査、仕様と試験の対応、変異試験、速さの測定、PR前の一括検査。
pub mod bench;
mod docs_check;
mod links;
mod mutate;
mod pipeline;
mod trace;
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// 指定されたリポジトリで点検を実行する。
///
/// # Errors
/// ファイルや子プロセスを扱えない場合、または引数が不正な場合にエラーを返す。
pub fn run(root: &Path, args: &[String]) -> Result<i32> {
    let temp_dir =
        std::env::var_os("MEIBO_XTASK_TMP").map_or_else(std::env::temp_dir, PathBuf::from);
    run_with_temp_dir(root, args, &temp_dir)
}

/// 一時作業フォルダの親を指定して点検を実行する。
///
/// # Errors
/// ファイルや子プロセスを扱えない場合、または引数が不正な場合にエラーを返す。
pub fn run_with_temp_dir(root: &Path, args: &[String], temp_dir: &Path) -> Result<i32> {
    let (command, rest) = args
        .split_first()
        .ok_or("使い方: cargo xtask <check-links|check-spec-trace|mutate|bench|docs-check|docs-render|check-all>")?;
    match command.as_str() {
        "check-links" => links::run(root),
        "check-spec-trace" => trace::run(root, rest.iter().any(|a| a == "--strict")),
        "mutate" => mutate::run(root, rest, temp_dir),
        "bench" => bench::run(root, rest),
        "bench-reader" => bench::reader_run(rest),
        "docs-check" | "docs-render" if rest.is_empty() => {
            docs_check::run(root, command == "docs-render")
        }
        "check-all" => Ok(pipeline::check_all(
            root,
            rest.first().is_some_and(|s| s == "--quick"),
        )),
        _ => Err(format!("不明なコマンド: {command}").into()),
    }
}
fn walk(root: &Path, excludes: &[&str]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.is_dir() {
        return Ok(files);
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if excludes
            .iter()
            .any(|s| path.file_name().is_some_and(|n| n == *s))
        {
            continue;
        }
        if path.is_dir() {
            files.extend(walk(&path, excludes)?);
        } else if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
fn command(root: &Path, program: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(root);
    if let Some(home) = std::env::var_os("HOME") {
        let mut paths = vec![PathBuf::from(home).join(".cargo/bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        if let Ok(path) = std::env::join_paths(paths) {
            cmd.env("PATH", path);
        }
    }
    cmd
}
fn output(root: &Path, program: &str, args: &[&str]) -> std::io::Result<Output> {
    command(root, program, args).output()
}
fn lines(text: &str) -> Vec<&str> {
    // \n・\r\n・\r のほか、垂直タブ・改ページ・Unicodeの行区切りも改行として扱う。末尾に空行は足さない。
    let mut result = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if matches!(
            ch,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}'
                ..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            if let Some(line) = text.get(start..offset) {
                result.push(line);
            }
            start = offset.saturating_add(ch.len_utf8());
            if ch == '\r'
                && chars.peek().is_some_and(|(_, c)| *c == '\n')
                && let Some((pos, c)) = chars.next()
            {
                start = pos.saturating_add(c.len_utf8());
            }
        }
    }
    if let Some(tail) = text.get(start..).filter(|s| !s.is_empty()) {
        result.push(tail);
    }
    result
}
