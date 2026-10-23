//! エンジンに依存しない速さの門。時間はミリ秒、日時は Unix epoch ミリ秒。
mod conditions;
mod engine;
mod reader;
mod runner;
mod search_cases;
use runner::Options;
#[cfg(test)]
#[path = "bench/tests.rs"]
mod engine_tests;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// 全目標の達成を述べるために必要な条件。
pub const REQUIRED_CONDITIONS: [&str; 2] = [
    "search-long-first/common-term",
    "search-common-count/common-term",
];

/// 測定する分布。
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// 公的名簿に近い疎な分布。
    Sparse,
    /// 購入リストに近い密な分布。
    Dense,
}
impl Profile {
    /// ファイル名にも使う分布名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::Dense => "dense",
        }
    }
}

/// 1つの測定条件。形や代表値ごとに id を分ける。
#[derive(Clone, Debug)]
pub struct Case {
    /// 記録間で変わらない識別子。
    pub id: String,
    /// 要件番号。
    pub req: String,
    /// 表示名。
    pub label: String,
    /// p95 の上限（ミリ秒）。
    pub target_ms: f64,
    /// 中央値の上限（ミリ秒）。
    pub p50_target_ms: Option<f64>,
    /// 30回以上の試行回数。
    pub trials: usize,
}
/// 目標値の唯一の表。Stage 4 はこれを形・代表値ごとに展開する。
#[must_use]
pub fn targets() -> Vec<Case> {
    [
        ("list-first", "3.1", "一覧の先頭50社", 10.0, None),
        ("list-deep", "3.2", "深い位置の次ページ", 10.0, None),
        ("count", "3.4", "宣言した形の厳密な件数", 100.0, None),
        ("slow", "3.5", "代表的な遅い条件", 2000.0, None),
        (
            "search-long-first",
            "4.4",
            "3文字以上の先頭50社",
            50.0,
            None,
        ),
        (
            "search-long-count",
            "4.4",
            "3文字以上の件数（当たりが5%以下の語）",
            100.0,
            None,
        ),
        (
            "search-common-count",
            "4.4",
            "3文字以上の件数（当たりが5%を超える語）",
            300.0,
            None,
        ),
        (
            "search-short-first",
            "4.5",
            "2文字以下の先頭50社",
            500.0,
            None,
        ),
        ("search-short-count", "4.5", "2文字以下の件数", 500.0, None),
        ("activity", "5.5", "活動1件の記録", 10.0, None),
        (
            "activity-reader",
            "5.6",
            "別プロセスの読み取り中の活動記録",
            10.0,
            None,
        ),
        ("create-list", "6.1", "10万社のリスト作成", 1000.0, None),
        (
            "bulk-update",
            "6.2",
            "10万社の一括変更",
            3000.0,
            Some(2000.0),
        ),
    ]
    .into_iter()
    .map(|(id, req, label, target_ms, p50_target_ms)| Case {
        id: id.into(),
        req: req.into(),
        label: label.into(),
        target_ms,
        p50_target_ms,
        trials: 30,
    })
    .collect()
}
/// 測定器の差し込み口。公開口の呼び出しから結果までの標本を返す。
pub trait Measurer {
    /// # Errors
    /// 測定できないときに理由を返す。
    fn measure(&mut self, case: &Case) -> Result<Vec<f64>, String>;
}
/// 測定環境。取得できない値は「不明」。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Env {
    /// OS とその版。
    pub os: String,
    /// CPU・機種。
    pub cpu: String,
    /// メモリ容量（GiB、文字列）。
    pub ram_gib: String,
    /// Rust の版。
    pub rust: String,
    /// SQLite の版。
    pub sqlite_version: String,
    /// 耐久性・キャッシュ・測定条件など。
    pub extra: BTreeMap<String, String>,
}
impl Default for Env {
    fn default() -> Self {
        Self {
            os: "不明".into(),
            cpu: "不明".into(),
            ram_gib: "不明".into(),
            rust: "不明".into(),
            sqlite_version: "不明".into(),
            extra: BTreeMap::new(),
        }
    }
}
/// 1条件の実測結果。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Measurement {
    /// 条件の識別子。
    pub id: String,
    /// 要件番号。
    pub req: String,
    /// 表示名。
    pub label: String,
    /// ミリ秒の全標本。長さが試行回数。
    pub samples: Vec<f64>,
    /// nearest-rank の50%値。
    pub p50_ms: f64,
    /// nearest-rank の95%値。
    pub p95_ms: f64,
    /// 最大値。
    pub max_ms: f64,
    /// 要件の目標値。
    pub target_ms: f64,
    /// 中央値の上限（ミリ秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_target_ms: Option<f64>,
    /// 目標以内か。
    pub pass: bool,
}
/// 保存する測定記録。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchRecord {
    /// 記録形式の版（現在1）。
    pub schema: u32,
    /// Unix epoch ミリ秒。
    pub created_at: String,
    /// 環境。
    pub env: Env,
    /// 分布。
    pub profile: Profile,
    /// 合成企業数。
    pub rows: u64,
    /// 合成データの乱数種。
    pub seed: u64,
    /// 条件ごとの結果。
    pub measurements: Vec<Measurement>,
}
fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or_else(|_| "不明".into(), |d| d.as_millis().to_string())
}
/// nearest-rank で (p50, p95, max) を計算する。
/// # Errors
/// 30標本未満、負値、NaN、無限大を拒否する。
pub fn percentiles(samples: &[f64]) -> Result<(f64, f64, f64), String> {
    if samples.len() < 30 || samples.iter().any(|x| !x.is_finite() || *x < 0.0) {
        return Err("有限な非負の標本が30回以上必要です".into());
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = |percent: usize| sorted[(percent * sorted.len()).div_ceil(100) - 1];
    Ok((rank(50), rank(95), rank(100)))
}
fn validate_cases(cases: &[Case]) -> Result<(), String> {
    if cases.is_empty() {
        return Err("測定条件が空です".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let table = targets();
    for c in cases {
        if c.id.is_empty()
            || !ids.insert(&c.id)
            || c.trials < 30
            || !table.iter().any(|t| {
                t.req == c.req
                    && t.target_ms.to_bits() == c.target_ms.to_bits()
                    && t.p50_target_ms.map(f64::to_bits) == c.p50_target_ms.map(f64::to_bits)
            })
        {
            return Err(format!(
                "測定条件が不正です: {}（ID・30試行・要件の目標を確認）",
                c.id
            ));
        }
    }
    Ok(())
}
/// 測定器から標本を受け取り目標と比較する。空き確認は呼び出し前に行う。
/// # Errors
/// 条件や標本が不正、または測定器が失敗したとき。
pub fn run_gate(
    measurer: &mut impl Measurer,
    cases: &[Case],
    env: Env,
    profile: Profile,
    rows: u64,
    seed: u64,
) -> Result<BenchRecord, String> {
    validate_cases(cases)?;
    if rows == 0 {
        return Err("合成企業数は1以上が必要です".into());
    }
    let mut measurements = Vec::new();
    for c in cases {
        let samples = measurer.measure(c)?;
        if samples.len() < c.trials {
            return Err(format!("{}: 指定試行回数に不足", c.id));
        }
        let (p50_ms, p95_ms, max_ms) = percentiles(&samples)?;
        measurements.push(Measurement {
            id: c.id.clone(),
            req: c.req.clone(),
            label: c.label.clone(),
            samples,
            p50_ms,
            p95_ms,
            max_ms,
            target_ms: c.target_ms,
            p50_target_ms: c.p50_target_ms,
            pass: meets_target(p50_ms, p95_ms, c.target_ms, c.p50_target_ms),
        });
    }
    Ok(BenchRecord {
        schema: 1,
        created_at: timestamp(),
        env,
        profile,
        rows,
        seed,
        measurements,
    })
}
fn meets_target(p50: f64, p95: f64, target: f64, p50_target: Option<f64>) -> bool {
    p95 <= target && p50 <= p50_target.unwrap_or(f64::INFINITY)
}
impl BenchRecord {
    /// 保存記録を再計算し、壊れた値や目標の改変を拒否する。
    /// # Errors
    /// 未対応の版、不正な条件や統計値のとき。
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.rows == 0 {
            return Err("記録の版または件数が不正です".into());
        }
        let cases: Vec<_> = self
            .measurements
            .iter()
            .map(|m| Case {
                id: m.id.clone(),
                req: m.req.clone(),
                label: m.label.clone(),
                target_ms: m.target_ms,
                p50_target_ms: m.p50_target_ms,
                trials: m.samples.len(),
            })
            .collect();
        validate_cases(&cases)?;
        for m in &self.measurements {
            let (p50, p95, max) = percentiles(&m.samples)?;
            if [p50, p95, max].map(f64::to_bits) != [m.p50_ms, m.p95_ms, m.max_ms].map(f64::to_bits)
                || m.pass != meets_target(p50, p95, m.target_ms, m.p50_target_ms)
            {
                return Err(format!("記録の統計値が標本と不一致: {}", m.id));
            }
        }
        Ok(())
    }
}
/// 差し替え可能な空き容量の確認。
pub trait SpaceProbe {
    /// # Errors
    /// ファイルシステムへの問い合わせに失敗したとき。
    fn available(&self, path: &Path) -> Result<u64, String>;
}
/// 実ファイルシステムの空きを使う。
pub struct FsSpaceProbe;
impl SpaceProbe for FsSpaceProbe {
    fn available(&self, path: &Path) -> Result<u64, String> {
        fs4::available_space(path).map_err(|e| e.to_string())
    }
}
/// 測定開始前の空き確認。
/// # Errors
/// 空き不足または容量の取得に失敗したとき。
pub fn ensure_space(path: &Path, needed_bytes: u64, probe: &impl SpaceProbe) -> Result<(), String> {
    let available = probe.available(path)?;
    if available < needed_bytes {
        return Err(format!(
            "空き容量不足: {} 必要{needed_bytes} bytes、空き{available} bytes",
            path.display()
        ));
    }
    Ok(())
}
/// 空き確認が通った後だけ測定器を生成・実行する。
/// # Errors
/// 空き不足、測定器の初期化失敗、測定失敗のとき。
pub fn with_space<T>(
    path: &Path,
    needed_bytes: u64,
    probe: &impl SpaceProbe,
    measure: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    ensure_space(path, needed_bytes, probe)?;
    measure()
}
/// 基準更新の監査記録。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BaselineChange {
    /// 更新理由。
    pub reason: String,
    /// Unix epoch ミリ秒。
    pub created_at: String,
    /// 更新前の全記録。
    pub old: BenchRecord,
}
/// 基準記録ファイル。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Baseline {
    /// 比較対象。
    pub record: BenchRecord,
    /// 明示更新の履歴。
    pub history: Vec<BaselineChange>,
}
/// 基準との比較結果。更新しても今回の不合格は取り消さない。
#[derive(Debug)]
pub struct GateOutcome {
    /// 全目標と基準比較を通ったか。
    pub pass: bool,
    /// 初回の基準を保存したか。
    pub initial_baseline: bool,
    /// 倍率・差分の両方を超えた条件ID。
    pub regressions: Vec<String>,
}
impl GateOutcome {
    /// コマンドの終了コード。目標未達または基準比悪化は1。
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        i32::from(!self.pass)
    }
}
/// 基準と比較し、初回または理由つきの明示更新時だけ基準を保存する。
/// # Errors
/// 理由が空、条件が不一致、記録不正、読み書き失敗のとき。
pub fn compare_baseline(
    root: &Path,
    current: &BenchRecord,
    update_reason: Option<&str>,
) -> Result<GateOutcome, String> {
    if update_reason.is_some_and(|r| r.trim().is_empty()) {
        return Err("基準更新には空でない理由が必要です".into());
    }
    current.validate()?;
    let path = root.join(format!(
        "docs/benchmarks/baseline-{}.json",
        current.profile.as_str()
    ));
    let previous = match fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice::<Baseline>(&bytes).map_err(|e| e.to_string())?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let meets_targets = current.measurements.iter().all(|m| m.pass);
    if !meets_targets {
        return Ok(GateOutcome {
            pass: false,
            initial_baseline: false,
            regressions: Vec::new(),
        });
    }
    let initial_baseline = previous.is_none();
    let mut regressions = Vec::new();
    if let Some(b) = &previous {
        let valid = b.record.validate();
        if let Err(error) = &valid
            && update_reason.is_none()
        {
            return Err(format!(
                "{error}。要件の変更で目標が変わった基準は --update-baseline --reason で置き換えられます"
            ));
        }
        let old = &b.record;
        let same_data =
            old.profile == current.profile && old.rows == current.rows && old.seed == current.seed;
        let compatible = same_data
            && old.measurements.len() == current.measurements.len()
            && current
                .measurements
                .iter()
                .all(|now| old.measurements.iter().any(|m| same_condition(m, now)));
        if !compatible && update_reason.is_none() {
            return Err("基準と分布・件数・種・条件数が不一致です。--update-baseline --reason で更新できます".into());
        }
        if valid.is_ok() && same_data {
            for now in &current.measurements {
                let Some(before) = old.measurements.iter().find(|m| same_condition(m, now)) else {
                    continue;
                };
                if now.p95_ms >= 2.0 * before.p95_ms && now.p95_ms - before.p95_ms >= 1.0 {
                    regressions.push(now.id.clone());
                }
            }
        }
    }
    if initial_baseline || update_reason.is_some() {
        let mut history = previous
            .as_ref()
            .map_or_else(Vec::new, |b| b.history.clone());
        if let (Some(before), Some(reason)) = (previous, update_reason) {
            history.push(BaselineChange {
                reason: reason.trim().into(),
                created_at: timestamp(),
                old: before.record,
            });
        }
        let baseline = Baseline {
            record: current.clone(),
            history,
        };
        fs::create_dir_all(path.parent().ok_or("基準ファイルの親がありません")?)
            .map_err(|e| e.to_string())?;
        fs::write(
            path,
            serde_json::to_vec_pretty(&baseline).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(GateOutcome {
        pass: current.measurements.iter().all(|m| m.pass) && regressions.is_empty(),
        initial_baseline,
        regressions,
    })
}
fn same_condition(a: &Measurement, b: &Measurement) -> bool {
    a.id == b.id
        && a.req == b.req
        && a.target_ms.to_bits() == b.target_ms.to_bits()
        && a.p50_target_ms.map(f64::to_bits) == b.p50_target_ms.map(f64::to_bits)
}
pub(super) fn run(root: &Path, args: &[String]) -> crate::Result<i32> {
    runner::run(root, &Options::parse(args)?)
}
pub(super) fn reader_run(args: &[String]) -> crate::Result<i32> {
    reader::run(args)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    struct Fake(f64);
    impl Measurer for Fake {
        fn measure(&mut self, c: &Case) -> Result<Vec<f64>, String> {
            Ok(vec![self.0; c.trials])
        }
    }
    pub(crate) fn record(value: f64) -> Result<BenchRecord, String> {
        run_gate(
            &mut Fake(value),
            &targets()[..1],
            Env::default(),
            Profile::Sparse,
            4_445_898,
            42,
        )
    }
    struct Samples(Vec<f64>);
    impl Measurer for Samples {
        fn measure(&mut self, _: &Case) -> Result<Vec<f64>, String> {
            Ok(self.0.clone())
        }
    }
    #[test]
    // REQ 6.2
    // REQ 9.2
    fn bulk_update_requires_both_percentile_targets() -> TestResult {
        let case = targets()
            .into_iter()
            .find(|c| c.id == "bulk-update")
            .ok_or("bulk")?;
        for (p50, p95, pass) in [
            (2000.0, 3000.0, true),
            (2000.1, 2500.0, false),
            (1900.0, 3000.1, false),
        ] {
            let mut samples = vec![p50; 15];
            samples.extend(vec![p95; 15]);
            let mut record = run_gate(
                &mut Samples(samples),
                std::slice::from_ref(&case),
                Env::default(),
                Profile::Sparse,
                100,
                42,
            )?;
            assert_eq!(record.measurements[0].pass, pass);
            record.validate()?;
            let dir = tempfile::tempdir()?;
            assert_eq!(
                compare_baseline(dir.path(), &record, None)?.exit_code(),
                i32::from(!pass)
            );
            record.measurements[0].pass = !pass;
            assert!(record.validate().is_err());
        }
        let mut altered = case;
        altered.p50_target_ms = None;
        assert!(
            run_gate(
                &mut Fake(1.0),
                &[altered],
                Env::default(),
                Profile::Sparse,
                1,
                42
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    // REQ 6.2
    fn legacy_measurement_without_median_target_deserializes() -> TestResult {
        let r = record(1.0)?;
        let json = serde_json::to_value(&r)?;
        assert!(json["measurements"][0].get("p50_target_ms").is_none());
        let copy: BenchRecord = serde_json::from_value(json)?;
        assert!(copy.measurements[0].p50_target_ms.is_none());
        copy.validate()?;
        Ok(())
    }
    #[test]
    // REQ 9.3
    fn obsolete_baseline_requires_reason_and_preserves_old_record() -> TestResult {
        let dir = tempfile::tempdir()?;
        let case = targets()
            .into_iter()
            .find(|c| c.id == "bulk-update")
            .ok_or("bulk")?;
        let current = run_gate(
            &mut Fake(3.0),
            &[case],
            Env::default(),
            Profile::Sparse,
            100,
            42,
        )?;
        compare_baseline(dir.path(), &current, None)?;
        let path = dir.path().join("docs/benchmarks/baseline-sparse.json");
        let mut baseline: Baseline = serde_json::from_slice(&fs::read(&path)?)?;
        let m = &mut baseline.record.measurements[0];
        m.target_ms = 2000.0;
        m.p50_target_ms = None;
        m.samples = vec![1.0; 30];
        m.p50_ms = 1.0;
        m.p95_ms = 1.0;
        m.max_ms = 1.0;
        assert!(!same_condition(
            &baseline.record.measurements[0],
            &current.measurements[0]
        ));
        let mut median_only = current.measurements[0].clone();
        median_only.p50_target_ms = None;
        assert!(!same_condition(&median_only, &current.measurements[0]));
        let original = serde_json::to_vec(&baseline)?;
        fs::write(&path, &original)?;
        let error = compare_baseline(dir.path(), &current, None).unwrap_err();
        assert!(error.contains("要件の変更で目標が変わった基準は --update-baseline --reason"));
        assert_eq!(fs::read(&path)?, original);
        let mut failed = current.clone();
        failed.measurements[0].samples = vec![3100.0; 30];
        let m = &mut failed.measurements[0];
        m.p50_ms = 3100.0;
        m.p95_ms = 3100.0;
        m.max_ms = 3100.0;
        m.pass = false;
        assert!(!compare_baseline(dir.path(), &failed, Some("承認済み"))?.pass);
        assert_eq!(fs::read(&path)?, original);
        let result = compare_baseline(dir.path(), &current, Some("承認済みの要件変更"))?;
        assert!(result.pass);
        assert_eq!(result.regressions, Vec::<String>::new());
        let updated: Baseline = serde_json::from_slice(&fs::read(path)?)?;
        assert_eq!(updated.history.len(), 1);
        assert_eq!(updated.history[0].reason, "承認済みの要件変更");
        assert_eq!(
            serde_json::to_value(&updated.history[0].old)?,
            serde_json::to_value(&baseline.record)?
        );
        updated.record.validate()?;
        Ok(())
    }
    #[test]
    // REQ 9.1
    fn nearest_rank_boundaries_and_invalid_samples() -> TestResult {
        for n in [0, 1, 29] {
            assert!(percentiles(&vec![1.0; n]).is_err());
        }
        for (n, expected) in [(30, (15.0, 29.0, 30.0)), (100, (50.0, 95.0, 100.0))] {
            let values: Vec<_> = (1..=n).rev().map(f64::from).collect();
            let actual = percentiles(&values)?;
            assert_eq!(
                [actual.0, actual.1, actual.2].map(f64::to_bits),
                [expected.0, expected.1, expected.2].map(f64::to_bits)
            );
        }
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(percentiles(&[bad; 30]).is_err());
        }
        Ok(())
    }
    #[test]
    // REQ 9.1
    // REQ 9.2
    fn exact_target_passes_and_excess_fails() -> TestResult {
        assert!(record(10.0)?.measurements[0].pass);
        assert!(!record(10.000_001)?.measurements[0].pass);
        let r = record(2.0)?;
        let copy: BenchRecord = serde_json::from_slice(&serde_json::to_vec(&r)?)?;
        copy.validate()?;
        assert_eq!(copy.rows, 4_445_898);
        assert_eq!(copy.seed, 42);
        assert_eq!(copy.profile, Profile::Sparse);
        assert_eq!(copy.measurements[0].samples.len(), 30);
        let mut bad = targets()[0].clone();
        bad.target_ms = 100.0;
        assert!(
            run_gate(
                &mut Fake(1.0),
                &[bad],
                Env::default(),
                Profile::Sparse,
                1,
                1
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    // REQ 9.3
    // REQ 9.2
    fn baseline_initial_comparison_and_history() -> TestResult {
        for (before, after, pass) in [
            (1.0, 2.0, false),
            (0.4, 0.8, true),
            (2.0, 3.0, true),
            (0.0, 0.0, true),
        ] {
            let dir = tempfile::tempdir()?;
            let initial = compare_baseline(dir.path(), &record(before)?, None)?;
            assert!(initial.initial_baseline);
            let path = dir.path().join("docs/benchmarks/baseline-sparse.json");
            let original = fs::read(&path)?;
            let result = compare_baseline(dir.path(), &record(after)?, None)?;
            assert_eq!(result.pass, pass);
            assert_eq!(result.exit_code(), i32::from(!pass));
            assert_eq!(fs::read(&path)?, original);
            assert!(!result.initial_baseline);
            assert!(compare_baseline(dir.path(), &record(after)?, Some(" ")).is_err());
            let updated = compare_baseline(
                dir.path(),
                &record(after)?,
                Some("承認済みの測定条件見直し"),
            )?;
            assert_eq!(updated.pass, pass);
            let baseline: Baseline = serde_json::from_slice(&fs::read(path)?)?;
            assert_eq!(baseline.history.len(), 1);
            assert_eq!(
                baseline.history[0].old.measurements[0].p95_ms.to_bits(),
                before.to_bits()
            );
            assert!(baseline.history[0].created_at.parse::<u128>()? > 0);
            assert_eq!(baseline.history[0].reason, "承認済みの測定条件見直し");
        }
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("docs/benchmarks/baseline-sparse.json");
        let rejected = compare_baseline(dir.path(), &record(11.0)?, None)?;
        assert_eq!(rejected.exit_code(), 1);
        assert!(!rejected.initial_baseline);
        assert!(!path.exists());
        assert_eq!(
            compare_baseline(dir.path(), &record(1.0)?, None)?.exit_code(),
            0
        );
        let original = fs::read(&path)?;
        assert!(!compare_baseline(dir.path(), &record(11.0)?, Some("approved"))?.pass);
        assert_eq!(original, fs::read(path)?);
        Ok(())
    }
    #[test]
    // REQ 9.3
    fn changed_conditions_require_reason_and_preserve_history() -> TestResult {
        for added in [false, true] {
            let dir = tempfile::tempdir()?;
            let old = record(1.0)?;
            compare_baseline(dir.path(), &old, None)?;
            let path = dir.path().join("docs/benchmarks/baseline-sparse.json");
            let original = fs::read(&path)?;
            let mut next = old.clone();
            let mut condition = next.measurements[0].clone();
            condition.id = "list-first/added".into();
            if added {
                next.measurements.push(condition);
            } else {
                next.measurements[0] = condition;
            }
            let error = compare_baseline(dir.path(), &next, None).unwrap_err();
            assert!(error.contains("--update-baseline --reason"));
            assert_eq!(fs::read(&path)?, original);
            let mut failed = next.clone();
            let m = &mut failed.measurements[0];
            m.samples = vec![11.0; 30];
            m.p50_ms = 11.0;
            m.p95_ms = 11.0;
            m.max_ms = 11.0;
            m.pass = false;
            assert_eq!(
                compare_baseline(dir.path(), &failed, Some("changed"))?.exit_code(),
                1
            );
            assert_eq!(fs::read(&path)?, original);
            assert_eq!(
                compare_baseline(dir.path(), &next, Some("changed"))?.exit_code(),
                0
            );
            let baseline: Baseline = serde_json::from_slice(&fs::read(path)?)?;
            assert_eq!(baseline.history.len(), 1);
            assert_eq!(baseline.history[0].reason, "changed");
            assert_eq!(
                serde_json::to_value(&baseline.history[0].old)?,
                serde_json::to_value(old)?
            );
            assert_eq!(
                serde_json::to_value(baseline.record)?,
                serde_json::to_value(next)?
            );
        }
        Ok(())
    }

    #[test]
    // REQ 9.3
    fn added_condition_still_compares_common_conditions() -> TestResult {
        for (value, pass) in [(1.0, true), (3.0, false)] {
            let dir = tempfile::tempdir()?;
            let old = record(1.0)?;
            compare_baseline(dir.path(), &old, None)?;
            let path = dir.path().join("docs/benchmarks/baseline-sparse.json");
            let original = fs::read(&path)?;
            let mut next = record(value)?;
            let mut added = next.measurements[0].clone();
            added.id = "list-first/added".into();
            next.measurements.push(added);
            let error = compare_baseline(dir.path(), &next, None).unwrap_err();
            assert!(error.contains("--update-baseline --reason"));
            assert_eq!(fs::read(&path)?, original);
            let outcome = compare_baseline(dir.path(), &next, Some("追加"))?;
            assert_eq!(outcome.pass, pass);
            assert_eq!(outcome.exit_code(), i32::from(!pass));
            assert_eq!(
                outcome.regressions,
                if pass { vec![] } else { vec!["list-first"] }
            );
            let baseline: Baseline = serde_json::from_slice(&fs::read(path)?)?;
            assert_eq!(
                serde_json::to_value(baseline.record)?,
                serde_json::to_value(next)?
            );
            assert_eq!(baseline.history.len(), 1);
            assert_eq!(baseline.history[0].reason, "追加");
            assert_eq!(
                serde_json::to_value(&baseline.history[0].old)?,
                serde_json::to_value(old)?
            );
        }
        Ok(())
    }
    #[test]
    // REQ 9.3
    fn changed_data_skips_common_condition_comparison() -> TestResult {
        for change in ["profile", "rows", "seed"] {
            let dir = tempfile::tempdir()?;
            let old = record(1.0)?;
            let mut next = record(3.0)?;
            match change {
                "profile" => next.profile = Profile::Dense,
                "rows" => next.rows += 1,
                _ => next.seed += 1,
            }
            let path = dir.path().join(format!(
                "docs/benchmarks/baseline-{}.json",
                next.profile.as_str()
            ));
            fs::create_dir_all(path.parent().ok_or("parent")?)?;
            fs::write(
                &path,
                serde_json::to_vec(&Baseline {
                    record: old.clone(),
                    history: vec![],
                })?,
            )?;
            assert!(compare_baseline(dir.path(), &next, None).is_err());
            let outcome = compare_baseline(dir.path(), &next, Some("データ変更"))?;
            assert_eq!(outcome.exit_code(), 0);
            assert_eq!(outcome.regressions, Vec::<String>::new());
            let baseline: Baseline = serde_json::from_slice(&fs::read(path)?)?;
            assert_eq!(
                serde_json::to_value(baseline.record)?,
                serde_json::to_value(next)?
            );
            assert_eq!(
                serde_json::to_value(&baseline.history[0].old)?,
                serde_json::to_value(old)?
            );
        }
        Ok(())
    }

    #[test]
    // REQ 4.4
    // REQ 9.2
    fn common_term_excess_fails_gate() -> TestResult {
        let dir = tempfile::tempdir()?;
        for (id, value, expected) in [
            ("search-long-first", 50.0, 0),
            ("search-long-first", 50.001, 1),
            ("search-long-count", 100.0, 0),
            ("search-long-count", 100.001, 1),
            ("search-common-count", 300.0, 0),
            ("search-common-count", 300.001, 1),
        ] {
            let mut case = targets().into_iter().find(|c| c.id == id).ok_or("target")?;
            case.id.push_str("/common-term");
            let r = run_gate(
                &mut Fake(value),
                &[case],
                Env::default(),
                Profile::Sparse,
                1,
                42,
            )?;
            assert_eq!(
                compare_baseline(dir.path(), &r, None)?.exit_code(),
                expected
            );
            // 合格時の基準を次の異なる条件と混ぜない。
            if expected == 0 {
                fs::remove_file(dir.path().join("docs/benchmarks/baseline-sparse.json"))?;
            }
        }
        Ok(())
    }
    #[test]
    // REQ 9.3
    fn incompatible_baseline_and_corrupt_record_fail() -> TestResult {
        let dir = tempfile::tempdir()?;
        compare_baseline(dir.path(), &record(1.0)?, None)?;
        let mut next = record(1.0)?;
        next.rows = 1;
        assert!(compare_baseline(dir.path(), &next, None).is_err());
        next = record(1.0)?;
        next.measurements[0].p95_ms = 0.0;
        assert!(next.validate().is_err());
        fs::write(
            dir.path().join("docs/benchmarks/baseline-sparse.json"),
            "broken",
        )?;
        assert!(compare_baseline(dir.path(), &record(1.0)?, None).is_err());
        Ok(())
    }
    #[test]
    // REQ 3.1
    // REQ 3.2
    // REQ 3.4
    // REQ 3.5
    // REQ 4.4
    // REQ 4.5
    // REQ 5.5
    // REQ 5.6
    // REQ 6.1
    // REQ 6.2
    fn targets_match_requirement_numbers() -> TestResult {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.kiro/specs/list-engine/requirements.md");
        let text = fs::read_to_string(path)?;
        let mut section = "";
        let mut reqs = BTreeMap::new();
        for line in text.lines() {
            if let Some(tail) = line.strip_prefix("### Requirement ") {
                section = tail.split(':').next().ok_or("要件番号なし")?;
            }
            if let Some((number, _)) = line
                .split_once(". ")
                .filter(|(n, _)| n.parse::<u32>().is_ok())
            {
                reqs.insert(format!("{section}.{number}"), line);
            }
        }
        let timing_values = |line: &str| -> Vec<f64> {
            line.split_inclusive("以内")
                .filter_map(|part| {
                    let end = part.strip_suffix("以内")?;
                    let (number, factor) = if let Some(s) = end.strip_suffix("ms") {
                        (s, 1.0)
                    } else {
                        (end.strip_suffix('秒')?, 1000.0)
                    };
                    let start = number
                        .rfind(|c: char| !c.is_ascii_digit() && c != '.')
                        .map_or(0, |i| {
                            i + number[i..].chars().next().map_or(0, char::len_utf8)
                        });
                    number[start..].parse::<f64>().ok().map(|n| n * factor)
                })
                .collect()
        };
        let table = targets();
        assert_eq!(table.len(), 13);
        for target in &table {
            let line = reqs.get(&target.req).ok_or("要件が見つからない")?;
            let values = if target.req == "5.6" {
                assert!(line.contains("前項の目標以内"));
                timing_values(reqs.get("5.5").ok_or("前項なし")?)
            } else {
                timing_values(line)
            };
            assert!(
                values
                    .iter()
                    .any(|v| v.to_bits() == target.target_ms.to_bits()),
                "{}: {values:?}",
                target.req
            );
        }
        let bulk = table.iter().find(|c| c.req == "6.2").ok_or("6.2なし")?;
        let bulk_values = timing_values(reqs.get("6.2").ok_or("6.2なし")?);
        assert_eq!(
            bulk.p50_target_ms.map(f64::to_bits),
            Some(bulk_values[0].to_bits())
        );
        // 同じ要件の各目標を確認する。
        let long: Vec<_> = table
            .iter()
            .filter(|c| c.req == "4.4")
            .map(|c| c.target_ms.to_bits())
            .collect();
        assert_eq!(
            long,
            timing_values(reqs.get("4.4").ok_or("4.4なし")?)
                .into_iter()
                .map(f64::to_bits)
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    #[test]
    // REQ 9.4
    fn insufficient_space_never_starts_measurement() {
        struct Probe;
        impl SpaceProbe for Probe {
            fn available(&self, _: &Path) -> Result<u64, String> {
                Ok(9)
            }
        }
        let mut started = false;
        let result = with_space(Path::new("unused"), 10, &Probe, || {
            started = true;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!started);
        assert!(ensure_space(Path::new("unused"), 9, &Probe).is_ok());
    }
    #[test]
    // REQ 9.2
    // REQ 9.3
    fn bench_arguments_and_nonofficial_baseline_rejection() -> TestResult {
        let args = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        for invalid in [
            "",
            "--profile other",
            "--profile sparse --rows 0",
            "--profile sparse --rows 4445899",
            "--profile sparse --target-ms 999",
            "--profile sparse --update-baseline",
            "--profile sparse --reason why",
            "--profile sparse --profile dense",
            "--profile sparse --workdir",
        ] {
            assert!(Options::parse(&args(invalid)).is_err(), "{invalid}");
        }
        for name in ["sparse", "dense"] {
            let dir = tempfile::tempdir()?;
            let mut valid = args(&format!("--profile {name} --rows 1 --workdir"));
            valid.push(dir.path().to_string_lossy().into_owned());
            valid.extend(args("--update-baseline --reason approved"));
            let parsed = Options::parse(&valid)?;
            assert_eq!(parsed.rows, 1);
            assert_eq!(parsed.profile.as_str(), name);
            assert_eq!(parsed.workdir, dir.path());
            assert!(run(dir.path(), &valid).is_err());
            assert_eq!(fs::read_dir(dir.path())?.count(), 0);
        }
        let mut empty_reason = args("--profile sparse --update-baseline --reason");
        empty_reason.push("   ".into());
        assert!(Options::parse(&empty_reason).is_err());
        Ok(())
    }
}
