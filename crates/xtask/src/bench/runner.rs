//! 実行条件、環境の記録、公式記録の保存。
use super::{
    BenchRecord, Env, FsSpaceProbe, Profile, compare_baseline, conditions,
    engine::{DatabaseFiles, EngineMeasurer, sizes},
    run_gate, timestamp, with_space,
};
use meibo_core::{
    Engine, Integrity, OpenOptions,
    synth::{Profile as SynthProfile, SynthConfig},
    types::DataOrigin,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

#[derive(Debug)]
pub(super) struct Options {
    pub profile: Profile,
    pub rows: u64,
    pub workdir: PathBuf,
    pub update_reason: Option<String>,
    pub keep_db: bool,
    pub only: Option<String>,
    pub allow_debug: bool,
    pub db: Option<PathBuf>,
}
impl Options {
    pub(super) fn parse(args: &[String]) -> Result<Self, String> {
        let mut profile = None;
        let mut rows = 4_445_898;
        let mut workdir = std::env::temp_dir();
        let mut update = false;
        let mut keep_db = false;
        let mut only = None;
        let mut allow_debug = false;
        let mut db = None;
        let mut reason = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if !seen.insert(arg) {
                return Err(format!("引数が重複: {arg}"));
            }
            if arg == "--keep-db" {
                keep_db = true;
                continue;
            }
            if arg == "--allow-debug" {
                allow_debug = true;
                continue;
            }
            if arg == "--update-baseline" {
                update = true;
                continue;
            }
            if !matches!(
                arg.as_str(),
                "--profile" | "--rows" | "--workdir" | "--reason" | "--only" | "--db"
            ) {
                return Err(format!("不明な引数: {arg}"));
            }
            let value = args
                .next()
                .filter(|s| !s.starts_with("--"))
                .ok_or_else(|| format!("{arg} に値が必要です"))?;
            match arg.as_str() {
                "--profile" => {
                    profile = Some(match value.as_str() {
                        "sparse" => Profile::Sparse,
                        "dense" => Profile::Dense,
                        _ => return Err("profile は sparse または dense".into()),
                    });
                }
                "--rows" => rows = value.parse::<u64>().map_err(|e| e.to_string())?,
                "--workdir" => workdir = PathBuf::from(value),
                "--reason" => reason = Some(value.clone()),
                "--only" => only = Some(value.clone()),
                "--db" => db = Some(PathBuf::from(value)),
                _ => unreachable!(),
            }
        }
        if rows == 0 || rows > 4_445_898 {
            return Err("rows は1〜4445898".into());
        }
        if update != reason.is_some() || reason.as_ref().is_some_and(|r| r.trim().is_empty()) {
            return Err("--update-baseline と空でない --reason を一緒に指定してください".into());
        }
        Ok(Self {
            profile: profile.ok_or("--profile sparse|dense が必要です")?,
            rows,
            workdir,
            update_reason: reason,
            keep_db,
            only,
            allow_debug,
            db,
        })
    }
}

impl Options {
    fn official(&self) -> bool {
        self.rows == 4_445_898
            && !cfg!(debug_assertions)
            && !self.allow_debug
            && self.only.is_none()
            && self.db.is_none()
    }
    fn validate(&self) -> Result<(), String> {
        if cfg!(debug_assertions) && !self.allow_debug {
            return Err(
                "測定は cargo xtask-release bench で実行してください（試験用: --allow-debug）"
                    .into(),
            );
        }
        if self.update_reason.is_some() && !self.official() {
            return Err("非公式の測定では基準を更新できません".into());
        }
        if self.rows == 4_445_898 && (self.only.is_some() || self.db.is_some()) {
            return Err("--only と --db は小規模な非公式測定専用です".into());
        }
        Ok(())
    }
}
fn command(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().replace('\n', " "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "不明".into())
}
fn environment() -> Env {
    let ram = command("sysctl", &["-n", "hw.memsize"]);
    let ram_gib = ram
        .parse::<u64>()
        .map_or_else(|_| "不明".into(), |n| format!("{}", n / (1 << 30)));
    let os = if cfg!(target_os = "macos") {
        command("sw_vers", &[])
    } else {
        command("uname", &["-sr"])
    };
    let mut env = Env {
        os,
        cpu: command("sysctl", &["-n", "machdep.cpu.brand_string"]),
        ram_gib,
        rust: command("rustc", &["-V"]),
        sqlite_version: Engine::sqlite_version().to_owned(),
        ..Env::default()
    };
    for (key, value) in [
        ("durability", "WAL/NORMAL"),
        ("reader_process_used", "false"),
        ("cache_mib", &OpenOptions::default().cache_mib.to_string()),
        (
            "background_checkpoint",
            &OpenOptions::default().background_checkpoint.to_string(),
        ),
        (
            "build",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        ),
        (
            "cache",
            "温かい状態（各条件の前に1回の暖機呼び出しを捨てる）",
        ),
        ("warmup", "1"),
        ("read_activity_samples", "100"),
        ("slow_list_bulk_samples", "30"),
        (
            "reader_process",
            "activity-reader only; Integrity::Never; search and list continuously",
        ),
        (
            "representative_selection",
            "all observed nonempty equality tuples; N uses observed UTC day ends; Engine::count ranks ascending with lexical tie break; lower median; unfiltered shapes have one tuple",
        ),
        (
            "multi_status",
            "10 entries, 6 distinct statuses (only 6 exist)",
        ),
    ] {
        env.extra.insert(key.into(), value.into());
    }
    env
}
fn write_record(path: &Path, record: &BenchRecord) -> crate::Result<()> {
    record.validate()?;
    fs::write(path, serde_json::to_vec_pretty(record)?)?;
    Ok(())
}
fn print_table(record: &BenchRecord) {
    println!("| 条件 | p50 ms | p95 ms | max ms | 目標 ms | 判定 |");
    println!("| --- | ---: | ---: | ---: | ---: | --- |");
    for m in &record.measurements {
        println!(
            "| {} | {:.6} | {:.6} | {:.6} | {} | {} |",
            m.id,
            m.p50_ms,
            m.p95_ms,
            m.max_ms,
            m.target_ms,
            if m.pass { "PASS" } else { "FAIL" }
        );
    }
}
pub(super) fn run(root: &Path, opts: &Options) -> crate::Result<i32> {
    opts.validate()?;
    fs::create_dir_all(&opts.workdir)?;
    let needed = opts
        .rows
        .checked_mul(1000)
        .and_then(|n| n.checked_add(2 << 30))
        .ok_or("必要容量が範囲外です")?;
    with_space(&opts.workdir, needed, &FsSpaceProbe, || Ok(()))?;
    let record = execute(opts)?;
    print_table(&record);
    let name = format!("run-{}.json", opts.profile.as_str());
    if opts.official() {
        fs::create_dir_all(root.join("docs/benchmarks"))?;
        write_record(&root.join("docs/benchmarks").join(&name), &record)?;
        let index_path = root.join("docs/benchmarks/latest.json");
        let mut index: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&index_path)?)?;
        index.insert(
            opts.profile.as_str().into(),
            format!("docs/benchmarks/{name}"),
        );
        fs::write(index_path, serde_json::to_vec_pretty(&index)?)?;
        crate::docs_check::run(root, true)?;
        let outcome = compare_baseline(root, &record, opts.update_reason.as_deref())?;
        if !outcome.regressions.is_empty() {
            eprintln!("基準比の悪化: {:?}", outcome.regressions);
        }
        Ok(outcome.exit_code())
    } else {
        write_record(&opts.workdir.join(name), &record)?;
        Ok(i32::from(record.measurements.iter().any(|m| !m.pass)))
    }
}
fn execute(opts: &Options) -> crate::Result<BenchRecord> {
    let mut env = environment();
    let directory = opts
        .workdir
        .join(format!("bench-{}-{}", timestamp(), std::process::id()));
    let cleanup = if opts.db.is_none() {
        fs::create_dir(&directory)?;
        Some(DatabaseFiles {
            directory: directory.clone(),
            keep: opts.keep_db,
        })
    } else {
        None
    };
    let path = opts
        .db
        .clone()
        .unwrap_or_else(|| directory.join("engine.db"));
    let start = Instant::now();
    let engine = open_engine(opts, &path, &mut env)?;
    env.extra.insert(
        "load_seconds".into(),
        start.elapsed().as_secs_f64().to_string(),
    );
    env.extra
        .insert("database_path".into(), path.display().to_string());
    env.extra
        .insert("reused_database".into(), opts.db.is_some().to_string());
    let (db, wal) = sizes(&path);
    env.extra
        .insert("loaded_database_bytes".into(), db.to_string());
    env.extra.insert("loaded_wal_bytes".into(), wal.to_string());
    eprintln!("読み込み完了。代表値の選定と全計画の検査を開始します");
    let selection_start = Instant::now();
    let mut selection = conditions::expand(&engine, opts.rows)?;
    env.extra.insert(
        "selection_seconds".into(),
        selection_start.elapsed().as_secs_f64().to_string(),
    );
    env.extra.insert("search_selection".into(), "synthetic fragments and pairs, optionally adjoining 株式会社; normalized substring histogram shortlists three per fraction; search_name counts choose nearest, lexical tie break".into());
    env.extra.append(&mut selection.extra);
    if let Some(prefix) = &opts.only {
        selection.prepared.retain(|p| p.case.id.starts_with(prefix));
    }
    if selection.prepared.is_empty() {
        return Err("--only に一致する条件がありません".into());
    }
    let cases: Vec<_> = selection.prepared.iter().map(|p| p.case.clone()).collect();
    let mut measurer = EngineMeasurer::new(engine, selection.prepared)?;
    eprintln!("計画検査成功。{}条件を測定します", cases.len());
    let mut record = run_gate(
        &mut measurer,
        &cases,
        env,
        opts.profile,
        opts.rows,
        20_261_022,
    )?;
    if !measurer.extra.contains_key("activity_full_p95_ms") {
        measurer.full_reference()?;
    }
    record.env.extra.append(&mut measurer.extra);
    let (db, wal) = sizes(&path);
    record
        .env
        .extra
        .insert("database_bytes".into(), db.to_string());
    record.env.extra.insert("wal_bytes".into(), wal.to_string());
    drop(measurer);
    drop(cleanup);
    record.validate()?;
    Ok(record)
}

fn open_engine(opts: &Options, path: &Path, env: &mut Env) -> crate::Result<Engine> {
    let engine = if opts.db.is_some() {
        let engine = Engine::open(
            path,
            &OpenOptions {
                integrity: Integrity::Never,
                ..OpenOptions::default()
            },
        )?;
        if engine.origin() != DataOrigin::Synthetic {
            return Err("合成データの保存先だけを再利用できます".into());
        }
        let n = engine
            .count(&meibo_core::types::Filters {
                include_excluded: true,
                ..Default::default()
            })?
            .n;
        if n != opts.rows {
            return Err(format!("--rows と保存先の社数が不一致: {n}").into());
        }
        engine
    } else {
        let mut engine = Engine::create(path, DataOrigin::Synthetic, &OpenOptions::default())?;
        let loaded = engine.load_synthetic(SynthConfig {
            rows: opts.rows,
            seed: 20_261_022,
            profile: match opts.profile {
                Profile::Sparse => SynthProfile::Sparse,
                Profile::Dense => SynthProfile::Dense,
            },
        })?;
        env.extra
            .insert("loaded_activities".into(), loaded.activities.to_string());
        engine
    };
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // REQ 9.1
    fn loaded_activities_only_for_fresh_database() -> crate::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("engine.db");
        let mut opts = Options::parse(&[
            "--profile".into(),
            "dense".into(),
            "--rows".into(),
            "1000".into(),
            "--allow-debug".into(),
        ])?;
        let mut env = Env::default();
        drop(open_engine(&opts, &path, &mut env)?);
        let loaded = env.extra["loaded_activities"].parse::<u64>()?;
        let expected: usize = meibo_core::synth::Synth::new(SynthConfig {
            rows: 1000,
            seed: 20_261_022,
            profile: SynthProfile::Dense,
        })
        .map(|r| r.activities.len())
        .sum();
        assert!(loaded > 0);
        assert_eq!(loaded, u64::try_from(expected)?);
        opts.db = Some(path.clone());
        let mut reused = Env::default();
        drop(open_engine(&opts, &path, &mut reused)?);
        assert!(!reused.extra.contains_key("loaded_activities"));
        Ok(())
    }
}
