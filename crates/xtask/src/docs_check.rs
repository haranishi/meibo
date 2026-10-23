use crate::{
    Result,
    bench::{BenchRecord, Measurement, Profile, REQUIRED_CONDITIONS},
};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    ops::Range,
    path::{Component, Path},
};
const BEGIN: &str = "<!-- bench:begin -->";
const END: &str = "<!-- bench:end -->";
const TEMPLATE: &str = "# 速さの測定\n\n合成データの予備値であり、実データの性能ではありません。\n記録から生成した結果だけを掲載します。\n\n同じ条件の測定を3回行い、10万社の一括変更の95%値が中央だった回を記録に採ります。ほかの回の記録は `.agent-harness/runs/2026-10-22-list-engine/official-runs-v3/` に残してあります（それ以前のコードや目標での測定は `official-runs-v2/` と `official-runs/`）。目標を満たさない条件は、緩めずにそのまま載せます。要件4.4と6.2を変えた経緯は [要件の変更の記録](../.kiro/specs/list-engine/amendments.md) にあります。\n\n<!-- bench:begin -->\n<!-- bench:end -->\n";

fn block(text: &str) -> Result<Range<usize>> {
    if text.matches(BEGIN).count() != 1 || text.matches(END).count() != 1 {
        return Err("測定ブロックの開始・終了マーカーが各1つ必要です".into());
    }
    let start = text.find(BEGIN).ok_or("開始マーカーなし")? + BEGIN.len();
    let end = text.find(END).ok_or("終了マーカーなし")?;
    if end < start {
        return Err("測定ブロックのマーカーが逆順です".into());
    }
    Ok(start..end)
}
fn cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\r', '\n'], " ")
        .replace('`', "&#96;")
}
fn read_records(root: &Path) -> Result<BTreeMap<String, (String, BenchRecord)>> {
    // 空の索引は未測定。索引の欠落や壊れた記録は未測定と扱わない。
    let index: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(root.join("docs/benchmarks/latest.json"))?)?;
    let mut records = BTreeMap::new();
    for (name, relative) in index {
        let expected = match name.as_str() {
            "sparse" => Profile::Sparse,
            "dense" => Profile::Dense,
            _ => return Err(format!("未知のプロファイル: {name}").into()),
        };
        let path = Path::new(&relative);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || !path.starts_with("docs/benchmarks")
        {
            return Err(format!("記録は docs/benchmarks 配下を指定: {relative}").into());
        }
        let resolved = root.join(path).canonicalize()?;
        if !resolved.starts_with(root.join("docs/benchmarks").canonicalize()?) {
            return Err("記録のリンク先が docs/benchmarks の外です".into());
        }
        let record: BenchRecord = serde_json::from_slice(&fs::read(resolved)?)?;
        record.validate()?;
        if record.profile != expected {
            return Err(format!("索引と記録のプロファイルが不一致: {name}").into());
        }
        records.insert(name, (relative, record));
    }
    Ok(records)
}
fn generated(root: &Path) -> Result<String> {
    let records = read_records(root)?;
    let mut out = String::from(
        "\n| 分布 | 社数 | 条件数 | 合格数 | 未達数 | 基準環境（Apple M4 Pro・48 GiB） | 結論 |\n| --- | ---: | ---: | ---: | ---: | --- | --- |\n",
    );
    for (name, label) in [("dense", "密"), ("sparse", "疎")] {
        if let Some((_, r)) = records.get(name) {
            let failed: Vec<_> = r.measurements.iter().filter(|m| !m.pass).collect();
            let conclusion = conclusion(&r.measurements);
            let baseline =
                if r.env.cpu == "Apple M4 Pro" && r.env.ram_gib.parse::<f64>() == Ok(48.0) {
                    "はい"
                } else {
                    "いいえ（記録値による判定）"
                };
            writeln!(
                out,
                "| {label}（{name}） | {} | {} | {} | {} | {baseline} | {conclusion} |",
                r.rows,
                r.measurements.len(),
                r.measurements.len() - failed.len(),
                failed.len()
            )?;
        } else {
            writeln!(out, "| {label}（{name}） | — | — | — | — | — | 未測定 |")?;
        }
    }
    if records
        .values()
        .any(|(_, r)| r.measurements.iter().any(|m| !m.pass))
    {
        out.push_str("\n未達の条件は目標を緩めて通さず、そのまま載せます。\n");
    }
    if records.is_empty() {
        out.push_str("\n測定記録はまだありません。\n");
    }
    for (name, description) in [
        ("dense", "密な分布（dense）\n\n購入リストに近い分布です。\n"),
        ("sparse", "疎な分布（sparse）\n\n公的名簿に近い分布です。\n"),
    ] {
        write!(out, "\n### {description}\n")?;
        if let Some((relative, record)) = records.get(name) {
            environment(&mut out, record, relative)?;
            group_table(&mut out, &record.measurements)?;
            references(&mut out, record)?;
            details(&mut out, &record.measurements)?;
        } else {
            out.push_str("未測定。\n");
        }
    }
    Ok(out)
}
fn conclusion(measurements: &[Measurement]) -> String {
    let mut parts = Vec::new();
    let failed: Vec<_> = measurements
        .iter()
        .filter(|m| !m.pass)
        .map(|m| cell(&m.id))
        .collect();
    if !failed.is_empty() {
        parts.push(format!("未達: {}", failed.join("、")));
    }
    let missing: Vec<_> = REQUIRED_CONDITIONS
        .iter()
        .filter(|id| !measurements.iter().any(|m| m.id == **id))
        .map(|id| cell(id))
        .collect();
    if !missing.is_empty() {
        parts.push(format!("未測定の条件あり: {}", missing.join("、")));
    }
    if parts.is_empty() {
        "全目標を満たした".into()
    } else {
        parts.join("。")
    }
}

fn os_name(raw: &str) -> String {
    let fields: Vec<_> = raw.split_whitespace().collect();
    if let [
        "ProductName:",
        "macOS",
        "ProductVersion:",
        version,
        "BuildVersion:",
        build,
    ] = fields.as_slice()
    {
        format!("macOS {version}（{build}）")
    } else {
        raw.into()
    }
}
fn utc_date(raw: &str) -> String {
    let Ok(ms) = raw.parse::<u64>() else {
        return raw.into();
    };
    // Gregorian civil date from epoch days; restrict output to four-digit years.
    if ms > 253_402_300_799_999 {
        return raw.into();
    }
    let days = i64::try_from(ms / 86_400_000).unwrap_or(0) + 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let seconds = ms / 1000 % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03} UTC",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        ms % 1000
    )
}
fn numeric(raw: &str, divisor: f64) -> String {
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map_or_else(|| cell(raw), |v| format!("{:.3}", v / divisor))
}
fn environment(out: &mut String, r: &BenchRecord, relative: &str) -> Result<()> {
    let e = &r.env;
    writeln!(
        out,
        "- 機種: {} / メモリ: {} GiB / OS: {}",
        cell(&e.cpu),
        cell(&e.ram_gib),
        cell(&os_name(&e.os))
    )?;
    writeln!(
        out,
        "- Rust: {} / SQLite: {}",
        cell(&e.rust),
        cell(&e.sqlite_version)
    )?;
    let settings: Vec<_> = [
        ("build", "ビルド"),
        ("durability", "耐久性"),
        ("cache", "キャッシュ"),
        ("cache_mib", "キャッシュ容量 (MiB)"),
        ("background_checkpoint", "背景のチェックポイント"),
        ("checkpoint", "チェックポイント"),
    ]
    .iter()
    .filter_map(|(key, label)| e.extra.get(*key).map(|v| format!("{label}: {}", cell(v))))
    .collect();
    if !settings.is_empty() {
        writeln!(out, "- {}", settings.join(" / "))?;
    }
    writeln!(
        out,
        "- 測定日時: {} / 社数: {} / 乱数の種: {}",
        cell(&utc_date(&r.created_at)),
        r.rows,
        r.seed
    )?;
    let mut storage = Vec::new();
    if let Some(v) = e.extra.get("load_seconds") {
        storage.push(format!("読み込み: {} 秒", numeric(v, 1.0)));
    }
    for (key, label) in [("database_bytes", "保存先（DB）"), ("wal_bytes", "WAL")] {
        if let Some(v) = e.extra.get(key) {
            storage.push(format!("{label}: {} GiB", numeric(v, 1_073_741_824.0)));
        }
    }
    if !storage.is_empty() {
        writeln!(out, "- {}", storage.join(" / "))?;
    }
    if let Some(n) = e.extra.get("loaded_activities") {
        writeln!(out, "- 読み込んだ活動: {}件", cell(n))?;
    }
    writeln!(out, "- 記録: {}", cell(relative))?;
    Ok(())
}
fn group_table(out: &mut String, measurements: &[Measurement]) -> Result<()> {
    let mut groups: Vec<(&str, Vec<&Measurement>)> = Vec::new();
    for m in measurements {
        let key = m.id.split('/').next().unwrap_or(&m.id);
        if let Some((_, members)) = groups.iter_mut().find(|(id, _)| *id == key) {
            members.push(m);
        } else {
            groups.push((key, vec![m]));
        }
    }
    out.push_str("\n| 目標群 | 要件 | 条件数 | 標本数/条件 | 最大 p95 (ms) | 目標 (ms) | 合否 | 最も遅い条件 |\n| --- | --- | ---: | --- | ---: | ---: | --- | --- |\n");
    for (_, members) in groups {
        let first = members[0];
        // 同率なら記録で先にある条件を選ぶ。
        let slowest = members
            .iter()
            .copied()
            .fold(first, |a, b| if b.p95_ms > a.p95_ms { b } else { a });
        let mut samples: Vec<_> = members.iter().map(|m| m.samples.len()).collect();
        samples.sort_unstable();
        samples.dedup();
        let samples = samples
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("、");
        writeln!(
            out,
            "| {} | {} | {} | {samples} | {:.3} | {} | {} | {} |",
            cell(&first.label),
            cell(&first.req),
            members.len(),
            slowest.p95_ms,
            target_label(first.target_ms, first.p50_target_ms),
            if members.iter().all(|m| m.pass) {
                "合格"
            } else {
                "未達"
            },
            cell(&slowest.id)
        )?;
    }
    Ok(())
}
fn references(out: &mut String, r: &BenchRecord) -> Result<()> {
    let extra = &r.env.extra;
    if extra.contains_key("activity_full_p95_ms")
        || extra.contains_key("bulk_update_back_to_back_p95_ms")
        || extra.contains_key("bulk_update_back_to_back_wal_bytes")
        || extra.contains_key("bulk_update_settle_max_ms")
    {
        out.push_str("\n参考値（目標の対象外）\n");
    }
    if let Some(ms) = extra.get("activity_full_p95_ms") {
        writeln!(
            out,
            "- 確定ごとに保存する設定（FULL）の活動1件のp95: {} ms。耐久性の設定を変えた場合の参考値です。",
            numeric(ms, 1.0)
        )?;
    }
    if let Some(ms) = extra.get("bulk_update_back_to_back_p95_ms") {
        writeln!(
            out,
            "- 重い一括変更を休まず繰り返した場合の p95（参考。書き戻しが追いつくまで待つ分が加わる）: {} ms。",
            numeric(ms, 1.0)
        )?;
    }
    if let Some(ms) = extra.get("bulk_update_settle_max_ms") {
        let timeouts = extra
            .get("bulk_update_settle_timeouts")
            .map_or("不明", String::as_str);
        writeln!(
            out,
            "- 一括変更の各回の前に待った書き戻しの最長（参考。保存先が止まると延びる）: {} ms、期限切れ: {}回。",
            numeric(ms, 1.0),
            cell(timeouts)
        )?;
    }
    if let Some(bytes) = extra.get("bulk_update_back_to_back_wal_bytes") {
        writeln!(
            out,
            "- 重い一括変更を休まず繰り返した間の WAL の最大の大きさ（参考）: {} バイト。",
            cell(bytes)
        )?;
    }
    Ok(())
}
fn target_label(p95: f64, p50: Option<f64>) -> String {
    p50.map_or_else(|| p95.to_string(), |p50| format!("{p95}（中央値 {p50}）"))
}
fn details(out: &mut String, measurements: &[Measurement]) -> Result<()> {
    write!(
        out,
        "\n<details><summary>全条件の値（{}条件）</summary>\n\n",
        measurements.len()
    )?;
    out.push_str("| 条件 | 要件 | 試行回数 | p50 (ms) | p95 (ms) | 最大 (ms) | 目標 (ms) | 合否 |\n| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for m in measurements {
        writeln!(
            out,
            "| {} ({}) | {} | {} | {:.3} | {:.3} | {:.3} | {} | {} |",
            cell(&m.label),
            cell(&m.id),
            cell(&m.req),
            m.samples.len(),
            m.p50_ms,
            m.p95_ms,
            m.max_ms,
            target_label(m.target_ms, m.p50_target_ms),
            if m.pass { "合格" } else { "未達" }
        )?;
    }
    out.push_str("\n</details>\n");
    Ok(())
}
// 時間の単位。数字の直後にこれが付いた記述を、測定の記録にない主張として検出する。
const TIMING_UNITS: [&str; 9] = [
    "ms",
    "us",
    "ミリ秒",
    "マイクロ秒",
    "μs",
    "µs",
    "ns",
    "秒",
    "s",
];
fn timing_prose(line: &str) -> String {
    let mut visible = String::new();
    let mut depth = 0usize;
    let mut previous = ' ';
    for c in line.chars() {
        if depth > 0 {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        } else if previous == ']' && c == '(' {
            depth = 1;
            visible.push(' ');
        } else {
            visible.push(c);
        }
        previous = c;
    }
    visible
        .split(|c: char| c.is_whitespace() || matches!(c, '（' | '）' | '(' | ')'))
        .filter(|token| {
            let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
            !token.contains('/')
                && ![".md", ".rs", ".json", ".toml", ".txt"]
                    .iter()
                    .any(|ext| token.ends_with(ext))
        })
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['*', '_', '`'], "")
}
fn has_timing(line: &str) -> bool {
    let normalized: String = timing_prose(line)
        .chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(u32::from(c) - u32::from('０') + u32::from('0'))
                .expect("ASCII digit"),
            '．' => '.',
            '，' => ',',
            _ => c,
        })
        .collect();
    for (i, c) in normalized.char_indices() {
        if !c.is_ascii_digit() {
            continue;
        }
        let tail = &normalized[i..];
        let number_end = tail
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
            .unwrap_or(tail.len());
        let suffix = tail[number_end..].trim_start();
        if TIMING_UNITS.iter().any(|unit| {
            suffix.strip_prefix(unit).is_some_and(|rest| {
                !rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        }) {
            return true;
        }
    }
    false
}
fn check_prose(name: &str, text: &str) -> Result<()> {
    let mut fence = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let ch = trimmed.chars().next().unwrap_or(' ');
        let count = trimmed.chars().take_while(|c| *c == ch).count();
        if let Some((open, len)) = fence {
            if ch == open && count >= len && trimmed[count..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if matches!(ch, '`' | '~') && count >= 3 {
            fence = Some((ch, count));
            continue;
        }
        if !line.contains("<!-- ok -->") && has_timing(line) {
            return Err(format!("{name}:{}: 記録の外に時間の数字があります", index + 1).into());
        }
    }
    Ok(())
}
pub(super) fn run(root: &Path, render: bool) -> Result<i32> {
    let path = root.join("docs/benchmarks.md");
    let mut text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if render && e.kind() == std::io::ErrorKind::NotFound => TEMPLATE.into(),
        Err(e) => return Err(e.into()),
    };
    if render && !text.contains(BEGIN) && !text.contains(END) {
        text.push_str("\n\n");
        text.push_str(TEMPLATE);
    }
    let range = block(&text)?;
    let expected = generated(root)?;
    if render {
        text.replace_range(range, &expected);
        fs::write(path, text)?;
        return Ok(0);
    }
    if text[range.clone()] != expected {
        let actual_lines: Vec<_> = text[range.clone()].split('\n').collect();
        let expected_lines: Vec<_> = expected.split('\n').collect();
        let first = (0..actual_lines.len().max(expected_lines.len()))
            .find(|i| actual_lines.get(*i) != expected_lines.get(*i))
            .unwrap_or(0);
        return Err(format!(
            "docs/benchmarks.md: 生成ブロックの最初の差分（ブロック内{}行目）\n実際: {}\n期待: {}",
            first + 1,
            actual_lines.get(first).unwrap_or(&"<EOF>"),
            expected_lines.get(first).unwrap_or(&"<EOF>")
        )
        .into());
    }
    let mut outside = text.clone();
    // 行数を保って報告する。
    outside.replace_range(
        range.clone(),
        &"\n".repeat(text[range].bytes().filter(|b| *b == b'\n').count()),
    );
    check_prose("docs/benchmarks.md", &outside)?;
    match fs::read_to_string(root.join("README.md")) {
        Ok(readme) => check_prose("README.md", &readme)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // 測定の記録から生成する benchmarks.md 以外の文書にも、記録にない時間の数字を書かせない。
    for doc in crate::walk(&root.join("docs"), &[])? {
        let is_md = doc.extension().is_some_and(|e| e == "md");
        if is_md && doc != path {
            let name = doc.strip_prefix(root).unwrap_or(&doc).display().to_string();
            check_prose(&name, &fs::read_to_string(&doc)?)?;
        }
    }
    let disclaimer = fs::read_to_string(root.join("DISCLAIMER.md"))?;
    check_prose("DISCLAIMER.md", &disclaimer)?;
    if !disclaimer.contains("電源断") {
        return Err("DISCLAIMER.md に「電源断」の説明がありません".into());
    }
    println!("docs-check: 記録と文書が一致しています");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        fs::create_dir_all(dir.path().join("docs/benchmarks"))?;
        fs::write(dir.path().join("docs/benchmarks/latest.json"), "{}")?;
        fs::write(
            dir.path().join("DISCLAIMER.md"),
            "電源断で失うことがあります",
        )?;
        run(dir.path(), true)?;
        Ok(dir)
    }
    #[test]
    // REQ 6.2
    // REQ 9.5
    fn median_target_in_both_tables_and_summary() -> Result<()> {
        struct Fixed;
        impl crate::bench::Measurer for Fixed {
            fn measure(&mut self, c: &crate::bench::Case) -> std::result::Result<Vec<f64>, String> {
                Ok(vec![if c.req == "6.2" { 2100.0 } else { 1.0 }; c.trials])
            }
        }
        let mut cases: Vec<_> = crate::bench::targets()
            .into_iter()
            .filter(|c| {
                c.id == "bulk-update"
                    || REQUIRED_CONDITIONS
                        .iter()
                        .any(|id| id.starts_with(&format!("{}/", c.id)))
            })
            .collect();
        for c in &mut cases {
            c.id.push_str(if c.req == "6.2" {
                "/default"
            } else {
                "/common-term"
            });
        }
        let record = crate::bench::run_gate(
            &mut Fixed,
            &cases,
            crate::bench::Env::default(),
            Profile::Sparse,
            100,
            42,
        )?;
        let mut out = String::new();
        group_table(&mut out, &record.measurements)?;
        assert!(out.contains("| 2100.000 | 3000（中央値 2000） | 未達 | bulk-update/default |"));
        out.clear();
        details(&mut out, &record.measurements)?;
        assert!(out.contains("| 2100.000 | 2100.000 | 2100.000 | 3000（中央値 2000） | 未達 |"));
        assert_eq!(
            conclusion(&record.measurements),
            "未達: bulk-update/default"
        );
        Ok(())
    }

    #[test]
    // REQ 9.5
    fn edited_block_fails() -> Result<()> {
        let dir = fixture()?;
        assert_eq!(run(dir.path(), false)?, 0);
        let path = dir.path().join("docs/benchmarks.md");
        fs::write(
            &path,
            fs::read_to_string(&path)?.replace("測定記録はまだありません。", "p95: 12 ms"),
        )?;
        assert!(run(dir.path(), false).is_err());
        Ok(())
    }
    #[test]
    // REQ 9.5
    fn prose_numbers_and_fences() -> Result<()> {
        let dir = fixture()?;
        fs::write(dir.path().join("docs/architecture.md"), "設計の判断\n")?;
        for name in [
            "docs/benchmarks.md",
            "README.md",
            "docs/architecture.md",
            "DISCLAIMER.md",
        ] {
            let path = dir.path().join(name);
            let original = fs::read_to_string(&path).unwrap_or_default();
            for number in [
                "検索p95は **2** ms",
                "_3_ 秒",
                "`4`ms",
                "検索は2 msで返る（docs/x.md）",
                "12.5 ms",
                "12ms",
                "12ミリ秒",
                "0.1秒",
                "p95は2秒",
                "5 µs",
                "3マイクロ秒",
            ] {
                fs::write(&path, format!("{original}\n{number}\n"))?;
                assert!(run(dir.path(), false).is_err());
            }
            for allowed in [
                "docs/2s.md [x](docs/2s.md) `crates/xtask/src/bench/2ms.rs`",
                "```rust\n12.5 ms\n```",
                "~~~~\n12ms\n~~~~",
                "12ミリ秒 <!-- ok -->",
            ] {
                fs::write(&path, format!("{original}\n{allowed}\n"))?;
                assert_eq!(run(dir.path(), false)?, 0);
            }
            fs::write(path, original)?;
        }
        Ok(())
    }
    #[test]
    // REQ 9.5
    fn timing_formats_and_non_timing_numbers() {
        for (text, expected) in [
            ("検索p95は **2** ms", true),
            ("_3_ 秒", true),
            ("`4`ms", true),
            ("docs/2s.md", false),
            ("[x](docs/2s.md)", false),
            ("`crates/xtask/src/bench/2ms.rs`", false),
            ("[x](2 ms)", false),
            ("[2 ms](docs/2s.md)", true),
            ("https://example.test/2ms", false),
            ("2s.md 3ms.rs 4s.json 5ms.toml 6s.txt", false),
            ("検索は2 msで返る（docs/x.md）", true),
            ("[x](docs/2s.md) 検索は **2** ms", true),
            ("2 seconds", false),
            ("2 s", true),
            ("2s", true),
            ("2 us", true),
            ("2us", true),
            ("２ms", true),
            ("１．５秒", true),
            ("1,500 ms", true),
            ("0.5秒", true),
            ("検索p95は2ms", true),
            ("Rust 1.88", false),
            ("SQLite 3.53.2", false),
            ("5.8", false),
            ("10万社", false),
            ("3回", false),
            ("95%値", false),
            ("2026-10-22", false),
            (
                ".agent-harness/runs/2026-10-22-list-engine/official-runs/",
                false,
            ),
            ("years tests docs status", false),
            ("2 status", false),
            ("2 settings", false),
            ("2 users", false),
        ] {
            assert_eq!(has_timing(text), expected, "{text}");
        }
    }

    #[test]
    // REQ 4.4
    // REQ 9.2
    // REQ 9.5
    fn common_term_failure_appears_in_summary_and_groups() -> Result<()> {
        struct Fixed;
        impl crate::bench::Measurer for Fixed {
            fn measure(&mut self, c: &crate::bench::Case) -> std::result::Result<Vec<f64>, String> {
                Ok(vec![
                    if c.id.starts_with("search-common-count") {
                        301.0
                    } else {
                        2.0
                    };
                    c.trials
                ])
            }
        }
        let dir = fixture()?;
        let cases: Vec<_> = crate::bench::targets()
            .into_iter()
            .filter(|c| c.req == "4.4")
            .map(|mut c| {
                c.id.push_str("/common-term");
                c
            })
            .collect();
        let record = crate::bench::run_gate(
            &mut Fixed,
            &cases,
            crate::bench::Env::default(),
            Profile::Sparse,
            1000,
            42,
        )?;
        save_record(dir.path(), &record)?;
        fs::write(
            dir.path().join("docs/benchmarks/latest.json"),
            r#"{"sparse":"docs/benchmarks/sparse.json"}"#,
        )?;
        let text = generated(dir.path())?;
        assert!(text.contains("未達: search-common-count/common-term"));
        assert!(text.contains("| 3文字以上の件数（当たりが5%を超える語） | 4.4 | 1 | 30 | 301.000 | 300 | 未達 | search-common-count/common-term |"));
        assert!(text.contains("| 3文字以上の先頭50社 | 4.4 | 1 | 30 | 2.000 | 50 | 合格 | search-long-first/common-term |"));
        assert!(!text.contains("よく使う語"));
        Ok(())
    }
    #[test]
    // REQ 9.5
    // REQ 4.4
    fn missing_required_conditions_are_not_full_success() -> Result<()> {
        let dir = measured_fixture()?;
        let text = generated(dir.path())?;
        assert!(!text.contains("全目標を満たした"));
        for id in REQUIRED_CONDITIONS {
            assert!(text.contains(id));
        }
        let mut record = crate::bench::tests::record(2.0)?;
        for (index, id) in REQUIRED_CONDITIONS.iter().enumerate() {
            let mut m = record.measurements[0].clone();
            m.id = (*id).into();
            m.req = "4.4".into();
            m.target_ms = if index == 0 { 50.0 } else { 300.0 };
            record.measurements.push(m);
        }
        save_record(dir.path(), &record)?;
        let text = generated(dir.path())?;
        assert!(text.contains(
            "| 疎（sparse） | 4445898 | 3 | 3 | 0 | いいえ（記録値による判定） | 全目標を満たした |"
        ));
        record.measurements.pop();
        save_record(dir.path(), &record)?;
        let text = generated(dir.path())?;
        assert!(!text.contains("全目標を満たした"));
        assert!(text.contains("未測定の条件あり: search-common-count/common-term |"));
        let mut legacy = record
            .measurements
            .last()
            .ok_or("first-page condition")?
            .clone();
        legacy.id = "search-long-count/common-term".into();
        legacy.target_ms = 100.0;
        record.measurements.push(legacy);
        save_record(dir.path(), &record)?;
        let text = generated(dir.path())?;
        assert!(!text.contains("全目標を満たした"));
        assert!(text.contains("未測定の条件あり: search-common-count/common-term |"));
        Ok(())
    }

    #[test]
    // REQ 9.5
    fn current_disclaimer_passes_prose_check() -> Result<()> {
        check_prose("DISCLAIMER.md", include_str!("../../../DISCLAIMER.md"))
    }

    #[test]
    // REQ 5.8
    fn missing_power_loss_notice() -> Result<()> {
        let dir = fixture()?;
        fs::write(dir.path().join("DISCLAIMER.md"), "利用上の注意")?;
        assert!(run(dir.path(), false).is_err());
        Ok(())
    }
    fn save_record(root: &Path, record: &BenchRecord) -> Result<()> {
        fs::write(
            root.join(format!("docs/benchmarks/{}.json", record.profile.as_str())),
            serde_json::to_vec(record)?,
        )?;
        Ok(())
    }
    fn measured_fixture() -> Result<tempfile::TempDir> {
        let dir = fixture()?;
        let mut sparse = crate::bench::tests::record(2.0)?;
        sparse.created_at = "1792677575800".into();
        sparse.env.cpu = "Apple M4 Pro".into();
        sparse.env.ram_gib = "48".into();
        sparse.env.os = "ProductName: macOS ProductVersion: 27.0.1 BuildVersion: 26A434".into();
        sparse.env.extra = [
            ("build", "release"),
            ("durability", "WAL/NORMAL"),
            ("cache", "warm"),
            ("background_checkpoint", "disabled"),
            ("load_seconds", "12.34567"),
            ("database_bytes", "1073741824"),
            ("activity_full_p95_ms", "0.12345"),
            ("bulk_update_back_to_back_p95_ms", "2345.6789"),
            ("bulk_update_back_to_back_wal_bytes", "54321000"),
            ("loaded_activities", "12345"),
            ("bulk_update_settle_max_ms", "432.1"),
            ("bulk_update_settle_timeouts", "2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let mut second = sparse.measurements[0].clone();
        second.id = "list-first/second".into();
        second.samples = vec![3.0; 40];
        second.p50_ms = 3.0;
        second.p95_ms = 3.0;
        second.max_ms = 3.0;
        sparse.measurements.push(second);
        let mut dense = sparse.clone();
        dense.profile = Profile::Dense;
        let mut failed = crate::bench::tests::record(11.0)?.measurements.remove(0);
        // 同じ目標群内の最大p95と合否を確認する。
        failed.id = "list-first/missed".into();
        dense.measurements.push(failed);
        save_record(dir.path(), &sparse)?;
        save_record(dir.path(), &dense)?;
        fs::write(
            dir.path().join("docs/benchmarks/latest.json"),
            r#"{"sparse":"docs/benchmarks/sparse.json","dense":"docs/benchmarks/dense.json"}"#,
        )?;
        Ok(dir)
    }
    #[test]
    // REQ 9.5
    fn records_render_and_match() -> Result<()> {
        let dir = measured_fixture()?;
        assert!(run(dir.path(), false).is_err());
        run(dir.path(), true)?;
        assert_eq!(run(dir.path(), false)?, 0);
        // 統計値の整合性を保つ変更でも文書との不一致を検出する。
        let path = dir.path().join("docs/benchmarks/dense.json");
        let mut record: BenchRecord = serde_json::from_slice(&fs::read(&path)?)?;
        record.seed += 1;
        save_record(dir.path(), &record)?;
        assert!(run(dir.path(), false).is_err());
        Ok(())
    }
    #[test]
    // REQ 9.5
    fn summary_counts_failures_and_group_maximum() -> Result<()> {
        let dir = measured_fixture()?;
        let text = generated(dir.path())?;
        assert!(
            text.contains("| 密（dense） | 4445898 | 3 | 2 | 1 | はい | 未達: list-first/missed。未測定の条件あり: search-long-first/common-term、search-common-count/common-term |")
        );
        assert!(text.contains("| 疎（sparse） | 4445898 | 2 | 2 | 0 | はい | 未測定の条件あり: search-long-first/common-term、search-common-count/common-term |"));
        assert!(text.contains(
            "| 一覧の先頭50社 | 3.1 | 3 | 30、40 | 11.000 | 10 | 未達 | list-first/missed |"
        ));
        assert!(text.contains("未達の条件は目標を緩めて通さず、そのまま載せます。"));
        assert!(text.contains("<details><summary>全条件の値（3条件）</summary>"));
        Ok(())
    }
    #[test]
    // REQ 9.1
    // REQ 9.5
    fn environment_is_readable_and_optional() -> Result<()> {
        let dir = measured_fixture()?;
        let text = generated(dir.path())?;
        assert!(!text.contains("{\""));
        for expected in [
            "macOS 27.0.1（26A434）",
            "Apple M4 Pro / メモリ: 48 GiB",
            "2026-10-22 13:59:35.800 UTC",
            "ビルド: release",
            "耐久性: WAL/NORMAL",
            "背景のチェックポイント: disabled",
            "読み込み: 12.346 秒",
            "保存先（DB）: 1.000 GiB",
            "読み込んだ活動: 12345件",
            "（FULL）の活動1件のp95: 0.123 ms",
            "書き戻しが追いつくまで待つ分が加わる）: 2345.679 ms",
            "保存先が止まると延びる）: 432.100 ms、期限切れ: 2回",
            "WAL の最大の大きさ（参考）: 54321000 バイト",
        ] {
            assert!(text.contains(expected), "missing {expected}");
        }
        let mut r = crate::bench::tests::record(2.0)?;
        r.env.os = "Other OS".into();
        r.created_at = "不明".into();
        let mut out = String::new();
        environment(&mut out, &r, "record.json")?;
        references(&mut out, &r)?;
        assert!(out.contains("OS: Other OS"));
        assert!(out.contains("測定日時: 不明"));
        for absent in [
            "読み込んだ活動:",
            "ビルド:",
            "読み込み:",
            "保存先",
            "参考値",
            "チェックポイント:",
        ] {
            assert!(!out.contains(absent));
        }
        Ok(())
    }
    #[test]
    // REQ 9.5
    fn single_distribution_is_unmeasured_and_generation_is_deterministic() -> Result<()> {
        let dir = measured_fixture()?;
        fs::write(
            dir.path().join("docs/benchmarks/latest.json"),
            r#"{"sparse":"docs/benchmarks/sparse.json"}"#,
        )?;
        let first = generated(dir.path())?;
        assert_eq!(first, generated(dir.path())?);
        assert!(first.contains("| 密（dense） | — | — | — | — | — | 未測定 |"));
        assert!(!first.contains("未達の条件は"));
        run(dir.path(), true)?;
        assert_eq!(run(dir.path(), false)?, 0);
        Ok(())
    }
    #[test]
    // REQ 9.5
    fn utc_dates_cover_epoch_leap_days_and_fallback() {
        assert_eq!(utc_date("0"), "1970-01-01 00:00:00.000 UTC");
        assert_eq!(utc_date("951868799999"), "2000-02-29 23:59:59.999 UTC");
        assert_eq!(utc_date("4107542400000"), "2100-03-01 00:00:00.000 UTC");
        assert_eq!(utc_date("253402300799999"), "9999-12-31 23:59:59.999 UTC");
        assert_eq!(utc_date("253402300800000"), "253402300800000");
        assert_eq!(utc_date("unknown"), "unknown");
    }
    #[test]
    // REQ 9.5
    fn invalid_index_and_markers_fail() -> Result<()> {
        let dir = fixture()?;
        for index in [
            r#"{"sparse":"../record.json"}"#,
            r#"{"unknown":"docs/benchmarks/no.json"}"#,
            "broken",
        ] {
            fs::write(dir.path().join("docs/benchmarks/latest.json"), index)?;
            assert!(run(dir.path(), true).is_err());
        }
        assert!(block("<!-- bench:end --><!-- bench:begin -->").is_err());
        assert!(block("<!-- bench:begin --><!-- bench:begin --><!-- bench:end -->").is_err());
        Ok(())
    }
}
