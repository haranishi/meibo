use crate::{Result, lines, walk};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

fn references(text: &str) -> Vec<String> {
    text.split("REQ ")
        .skip(1)
        .filter_map(|tail| {
            let n = tail.bytes().take_while(u8::is_ascii_digit).count();
            if n == 0 || tail.get(n..n + 1) != Some(".") {
                return None;
            }
            let m = tail[n + 1..].bytes().take_while(u8::is_ascii_digit).count();
            if m == 0 || tail[n + 1 + m..].starts_with(|c: char| c.is_alphanumeric() || c == '.') {
                return None;
            }
            Some(tail[..n + 1 + m].to_owned())
        })
        .collect()
}
fn criteria(text: &str) -> Vec<String> {
    let mut requirement = None;
    let mut found = Vec::new();
    for line in lines(text) {
        if let Some(tail) = line.strip_prefix("### Requirement ") {
            requirement = tail
                .split_once(':')
                .and_then(|(n, _)| n.parse::<u32>().ok());
        } else if line.starts_with('#') {
            requirement = None;
        }
        if let Some(n) = requirement
            && let Some((m, tail)) = line.trim_start().split_once(". ")
            && !tail.is_empty()
            && m.parse::<u32>().is_ok()
        {
            found.push(format!("{n}.{m}"));
        }
    }
    found
}
fn test_references(root: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in walk(&root.join("crates"), &["target", ".git"])? {
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let rel = path.strip_prefix(root)?;
        // `tests/` の結合試験と、`#[cfg(test)] mod tests;` で分けた `tests.rs` は丸ごと試験。
        let integration = rel
            .components()
            .any(|p| p.as_os_str() == "tests" || p.as_os_str() == "tests.rs");
        let mut unit_indent = None;
        let mut pending = Vec::new();
        let mut current = String::new();
        for (i, line) in lines(&text).iter().enumerate() {
            let indent = line.len() - line.trim_start().len();
            if line.trim() == "#[cfg(test)]" {
                unit_indent = Some(indent);
            }
            if line.trim() == "}" && unit_indent == Some(indent) {
                unit_indent = None;
                pending.clear();
            }
            if !integration && unit_indent.is_none() {
                continue;
            }
            if let Some(tail) = line.trim().strip_prefix("fn ") {
                tail.split('(')
                    .next()
                    .unwrap_or_default()
                    .clone_into(&mut current);
                for id in pending.drain(..) {
                    found.entry(id).or_default().push(format!(
                        "{}:{} {current}",
                        rel.display(),
                        i + 1
                    ));
                }
            }
            for id in references(line) {
                if line.trim().starts_with("//") {
                    pending.push(id);
                } else {
                    found.entry(id).or_default().push(format!(
                        "{}:{} {current}",
                        rel.display(),
                        i + 1
                    ));
                }
            }
        }
    }
    Ok(found)
}
pub(super) fn run(root: &Path, strict: bool) -> Result<i32> {
    let mut count = 0;
    let mut missing = Vec::new();
    let places = test_references(root)?;
    let mut known = BTreeSet::new();
    for path in walk(&root.join(".kiro/specs"), &[])? {
        if path.file_name().is_none_or(|n| n != "requirements.md") {
            continue;
        }
        for id in criteria(&fs::read_to_string(&path)?) {
            count += 1;
            known.insert(id.clone());
            let refs = places.get(&id).map_or(&[][..], Vec::as_slice);
            println!(
                "{id}: {}",
                if refs.is_empty() {
                    "未対応".to_owned()
                } else {
                    refs.join(", ")
                }
            );
            if refs.is_empty() {
                missing.push(id);
            }
        }
    }
    for id in places.keys().filter(|id| !known.contains(*id)) {
        println!("仕様に無い注記: {id}");
    }
    println!("基準 {count}件、未対応 {}件", missing.len());
    Ok(i32::from(strict && !missing.is_empty()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_criteria_and_markers() {
        assert_eq!(
            criteria("### Requirement 10: x\n1. A\n12. B\n## other\n1. no"),
            ["10.1", "10.12"]
        );
        let marker = ["RE", "Q "].concat();
        assert_eq!(
            references(&format!("// {marker}10.12, {marker}1.1")),
            ["10.12", "1.1"]
        );
        assert_eq!(
            references(&format!("{marker}.1 {marker}1. {marker}1.2x {marker}1.2.3")),
            Vec::<String>::new()
        );
    }
    #[test]
    fn strict_reports_missing_and_ignores_production_markers() {
        let root = tempfile::tempdir().unwrap();
        let marker = ["RE", "Q "].concat();
        for (name, content) in [
            (
                ".kiro/specs/a/requirements.md",
                "### Requirement 1: x\n1. A\n2. B".to_owned(),
            ),
            (
                "crates/a/src/lib.rs",
                format!(
                    "// {marker}1.2\n#[cfg(test)]\nmod tests {{\n    // {marker}1.1\n    fn works() {{}}\n}}\n// {marker}1.2\nfn production() {{}}"
                ),
            ),
        ] {
            let p = root.path().join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        assert_eq!(run(root.path(), false).unwrap(), 0);
        assert_eq!(run(root.path(), true).unwrap(), 1);
        assert!(test_references(root.path()).unwrap()["1.1"][0].ends_with("works"));
        // 別ファイルの単体試験（tests.rs）の注記は試験として数える。
        let split = root.path().join("crates/a/src/m/tests.rs");
        fs::create_dir_all(split.parent().unwrap()).unwrap();
        fs::write(split, format!("// {marker}1.2\nfn separate() {{}}")).unwrap();
        assert_eq!(run(root.path(), true).unwrap(), 0);
        assert!(test_references(root.path()).unwrap()["1.2"][0].ends_with("separate"));
    }
}
