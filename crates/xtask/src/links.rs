use crate::{Result, lines, output, walk};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};
const SKIP: &[&str] = &[
    ".agent-harness",
    ".git",
    ".cargo-home",
    ".cargo-target",
    "target",
    "node_modules",
];
fn private(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "research")
        && path.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n.starts_with("out-") || n.starts_with("notes_")
        })
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let git = output(root, "git", &["ls-files", "-o", "-c", "--exclude-standard"])?;
    if git.status.success() && root.join(".git").exists() {
        Ok(lines(&String::from_utf8_lossy(&git.stdout))
            .iter()
            .map(|s| root.join(s))
            .filter(|p| p.is_file())
            .collect())
    } else {
        Ok(walk(root, SKIP)?
            .into_iter()
            .filter(|p| !private(p))
            .collect())
    }
}
fn targets(line: &str) -> Vec<&str> {
    let mut remaining = line;
    let mut result = Vec::new();
    while let Some((_, tail)) = remaining.split_once("](") {
        if let Some((target, after)) = tail.split_once(')')
            && !target.is_empty()
            && !target.chars().any(char::is_whitespace)
        {
            result.push(target);
            remaining = after;
        } else {
            remaining = tail;
        }
    }
    result
}
fn external(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}
fn resolve(path: &Path) -> PathBuf {
    // 最後の要素が存在しなくても、途中にあるシンボリックリンクを解決する。
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => (),
            part => {
                result.push(part);
                if let Ok(canonical) = result.canonicalize() {
                    result = canonical;
                }
            }
        }
    }
    result
}
pub(super) fn run(root: &Path) -> Result<i32> {
    let files = files(root)?;
    let mut problems = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(root)?.display();
        if path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().contains(['#', '|', '[', ']', '^']))
        {
            problems.push(format!("{rel}: ファイル名に使えない文字があります"));
        }
    }
    let mut markdown = 0usize;
    for path in &files {
        if path.extension().is_none_or(|s| s != "md") {
            continue;
        }
        markdown = markdown.saturating_add(1);
        let rel = path.strip_prefix(root)?.to_string_lossy();
        let text = fs::read_to_string(path)?;
        let mut fenced = false;
        for (number, line) in lines(&text).into_iter().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if fenced {
                continue;
            }
            for target in targets(line) {
                if external(target) || target.starts_with('#') {
                    continue;
                }
                let dest = resolve(
                    &path
                        .parent()
                        .ok_or("文書の親がありません")?
                        .join(target.split('#').next().unwrap_or_default()),
                );
                if !dest.exists() {
                    problems.push(format!(
                        "{rel}:{}: リンク切れ {target}",
                        number.saturating_add(1)
                    ));
                } else if output(
                    root,
                    "git",
                    &["check-ignore", "-q", &dest.to_string_lossy()],
                )?
                .status
                .success()
                {
                    problems.push(format!(
                        "{rel}:{}: 公開されないファイルへのリンク {target}",
                        number.saturating_add(1)
                    ));
                }
            }
        }
        // 測定の記録から機械的に作る docs/benchmarks.md は対象外（要約を先頭に置き、全条件の表は折りたたむ）。
        let dev = (rel == "README.md" || rel == "CONTRIBUTING.md" || rel.starts_with("docs/"))
            && !rel.starts_with("docs/images/")
            && rel != "docs/benchmarks.md";
        let count = lines(&text).len();
        if dev && count > 100 {
            problems.push(format!("{rel}: {count}行あります(上限100行)"));
        }
    }
    for problem in &problems {
        println!("{problem}");
    }
    println!("検査したMarkdown: {markdown}件、問題: {}件", problems.len());
    Ok(i32::from(!problems.is_empty()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_and_lines() {
        assert_eq!(
            targets("[a](one#x) [b](two) [x](bad space)"),
            vec!["one#x", "two"]
        );
        assert!(external("https://example.org"));
        assert!(!external("HTTP:x"));
        assert_eq!(lines("a\r\nb\u{2028}\n"), vec!["a", "b", ""]);
        assert!(private(Path::new("research/out-a.md")));
    }
}
