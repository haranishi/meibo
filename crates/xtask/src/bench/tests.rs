use super::*;
use meibo_core::{
    Engine, OpenOptions,
    synth::{Profile as SynthProfile, SynthConfig},
    types::Filters,
};
use std::{collections::BTreeSet, process::Command};
type TestResult = crate::Result<()>;

fn database(path: &Path, profile: SynthProfile) -> crate::Result<Engine> {
    let mut engine = Engine::create(
        path,
        meibo_core::types::DataOrigin::Synthetic,
        &OpenOptions::default(),
    )?;
    engine.load_synthetic(SynthConfig {
        rows: 20_000,
        seed: 20_261_022,
        profile,
    })?;
    Ok(engine)
}

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
// REQ 9.1
#[test]
fn expansion_has_unique_ids_targets_all_shapes_and_categories() -> TestResult {
    let dir = tempfile::tempdir()?;
    let engine = database(&dir.path().join("engine.db"), SynthProfile::Dense)?;
    let selection = conditions::expand(&engine, 20_000)?;
    let cases: Vec<_> = selection.prepared.iter().map(|p| p.case.clone()).collect();
    validate_cases(&cases)?;
    let ids: BTreeSet<_> = cases.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), cases.len());
    for number in 1..=22 {
        for size in ["large", "median"] {
            assert!(ids.contains(format!("list-first/s{number:02}/{size}").as_str()));
            assert!(ids.contains(format!("count/s{number:02}/{size}").as_str()));
        }
    }
    for name in ["prefecture", "municipality", "industry", "status", "owner"] {
        assert!(ids.contains(format!("list-first/multi/{name}").as_str()));
        assert!(ids.contains(format!("count/multi/{name}").as_str()));
    }
    for name in ["pt", "it", "pit", "two-multi", "owner"] {
        assert!(ids.contains(format!("slow/{name}/first").as_str()));
        assert!(ids.contains(format!("slow/{name}/count").as_str()));
    }
    for target in targets() {
        assert!(
            cases
                .iter()
                .any(|c| c.id.starts_with(&format!("{}/", target.id)))
        );
    }
    for case in cases {
        assert_eq!(
            case.trials,
            if matches!(case.req.as_str(), "3.5" | "6.1" | "6.2") {
                30
            } else {
                100
            }
        );
    }
    Ok(())
}

// REQ 4.4
// REQ 9.1
// REQ 9.2
// REQ 9.5
// REQ 5.5
// REQ 5.6
// REQ 6.1
// REQ 6.2
#[test]
fn small_runs_both_profiles_validate_cleanup_and_leave_docs_untouched() -> TestResult {
    for profile in ["sparse", "dense"] {
        let dir = tempfile::tempdir()?;
        let docs = dir.path().join("docs/benchmarks");
        fs::create_dir_all(&docs)?;
        fs::write(docs.join("latest.json"), "sentinel")?;
        let args = vec![
            "--profile".into(),
            profile.into(),
            "--rows".into(),
            "20000".into(),
            "--workdir".into(),
            dir.path().display().to_string(),
            "--allow-debug".into(),
        ];
        let code = run(dir.path(), &args)?;
        let record: BenchRecord =
            serde_json::from_slice(&fs::read(dir.path().join(format!("run-{profile}.json")))?)?;
        record.validate()?;
        assert_eq!(code, i32::from(record.measurements.iter().any(|m| !m.pass)));
        for (prefix, n) in [
            ("activity/", 100),
            ("activity-reader/", 100),
            ("create-list/", 30),
            ("bulk-update/", 30),
        ] {
            assert_eq!(
                record
                    .measurements
                    .iter()
                    .find(|m| m.id.starts_with(prefix))
                    .ok_or("missing write samples")?
                    .samples
                    .len(),
                n
            );
        }
        assert!(record.env.extra["reader_iterations"].parse::<u64>()? > 0);
        assert!(
            record.env.extra["activity_full_p95_ms"]
                .parse::<f64>()?
                .is_finite()
        );
        assert!(!record.env.extra.contains_key("search_common_term_count_ms"));
        search_and_bulk_evidence(&record, profile)?;
        assert!(record.env.extra["bulk_update_settle"].contains("各回の前"));
        assert!(
            record.env.extra["bulk_update_back_to_back_p95_ms"]
                .parse::<f64>()?
                .is_finite()
        );
        assert!(record.env.extra["bulk_update_back_to_back_wal_bytes"].parse::<u64>()? > 0);
        assert!(!Path::new(&record.env.extra["database_path"]).exists());
        assert!(!fs::read_dir(dir.path())?.any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("bench-")
        }));
        assert_eq!(fs::read_to_string(docs.join("latest.json"))?, "sentinel");
        assert_eq!(fs::read_dir(docs)?.count(), 1);
    }
    Ok(())
}

fn search_and_bulk_evidence(record: &BenchRecord, profile: &str) -> TestResult {
    assert!(record.env.extra["loaded_activities"].parse::<u64>()? > 0);
    assert!(record.env.extra["search/common-term"].contains("term=株式会社; count="));
    for (id, target) in [
        ("search-long-first/common-term", 50.0_f64),
        ("search-common-count/common-term", 300.0_f64),
        ("search-common-count/limited-company", 300.0_f64),
        ("search-long-first/limited-company", 50.0_f64),
    ] {
        let m = record
            .measurements
            .iter()
            .find(|m| m.id == id)
            .ok_or("common term missing")?;
        assert_eq!(m.req, "4.4");
        assert_eq!(m.target_ms.to_bits(), target.to_bits());
        assert_eq!(m.samples.len(), 100);
    }
    assert!(record.env.extra["search/limited-company"].contains("term=有限会社; count="));
    for id in [
        "search-common-count/common-term",
        "search-common-count/limited-company",
        "search-long-first/limited-company",
        "bulk-update/default",
    ] {
        let m = record
            .measurements
            .iter()
            .find(|m| m.id == id)
            .ok_or("missing evidence")?;
        println!("STAGE4M {profile}: {}", serde_json::to_string(m)?);
    }
    let bulk = record
        .measurements
        .iter()
        .find(|m| m.id == "bulk-update/default")
        .ok_or("bulk")?;
    assert_eq!(bulk.target_ms.to_bits(), 3000.0_f64.to_bits());
    assert_eq!(
        bulk.p50_target_ms.map(f64::to_bits),
        Some(2000.0_f64.to_bits())
    );
    Ok(())
}

// REQ 3.1
// REQ 9.2
#[test]
fn missing_index_fails_before_measurement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("engine.db");
    drop(database(&path, SynthProfile::Dense)?);
    assert!(
        Command::new("sqlite3")
            .arg(&path)
            .arg("DROP INDEX k2")
            .status()?
            .success()
    );
    let before = Command::new("sqlite3")
        .arg(&path)
        .arg("SELECT COUNT(*) FROM activities")
        .output()?
        .stdout;
    let args = vec![
        "--profile".into(),
        "dense".into(),
        "--rows".into(),
        "20000".into(),
        "--workdir".into(),
        dir.path().display().to_string(),
        "--allow-debug".into(),
        "--db".into(),
        path.display().to_string(),
    ];
    assert!(run(dir.path(), &args).is_err());
    assert!(!dir.path().join("run-dense.json").exists());
    let after = Command::new("sqlite3")
        .arg(&path)
        .arg("SELECT COUNT(*) FROM activities")
        .output()?
        .stdout;
    assert_eq!(before, after);
    Ok(())
}

// REQ 5.6
#[test]
fn reader_starts_and_stdin_eof_finishes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("engine.db");
    let engine = database(&path, SynthProfile::Sparse)?;
    let reader = reader::Reader::start(&path)?;
    assert!(engine.count(&Filters::default())?.n > 0);
    assert!(reader.finish()? > 0);
    Ok(())
}

// REQ 9.1
// REQ 9.2
#[test]
fn debug_requires_explicit_override_and_partial_runs_cannot_update_baseline() -> TestResult {
    let dir = tempfile::tempdir()?;
    let args = vec![
        "--profile".into(),
        "dense".into(),
        "--rows".into(),
        "20000".into(),
        "--workdir".into(),
        dir.path().display().to_string(),
    ];
    if cfg!(debug_assertions) {
        let error = run(dir.path(), &args).unwrap_err().to_string();
        assert!(error.contains("cargo xtask-release bench"));
        assert_eq!(fs::read_dir(dir.path())?.count(), 0);
    }
    let mut args = args;
    args.extend([
        "--allow-debug".into(),
        "--update-baseline".into(),
        "--reason".into(),
        "test".into(),
    ]);
    assert!(run(dir.path(), &args).is_err());
    assert_eq!(fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}
