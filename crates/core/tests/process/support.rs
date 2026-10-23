use meibo_core::{
    Engine, OpenOptions,
    synth::{Profile, Synth, SynthConfig},
    types::{
        Activity, ActivityInput, ActivityKind, CompanyId, DataOrigin, Field, Filters, ListQuery,
        NextAction, SearchOptions, Sort, Status,
    },
};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::Duration,
};

pub const TIMEOUT: Duration = Duration::from_secs(120);
pub const DB_ENV: &str = "MEIBO_PROCESS_DB";

pub fn config(rows: u64) -> SynthConfig {
    SynthConfig {
        rows,
        seed: 42,
        profile: Profile::Dense,
    }
}

pub fn seed(path: &Path, rows: u64, empty_activities: bool) -> u64 {
    let mut db = Engine::create(path, DataOrigin::Synthetic, &OpenOptions::default()).unwrap();
    if empty_activities {
        for mut record in Synth::new(config(rows)) {
            record.company.status = Status::Untouched;
            db.register_company(&record.company).unwrap();
        }
        0
    } else {
        db.load_synthetic(config(rows)).unwrap().activities
    }
}

pub fn query() -> ListQuery {
    ListQuery {
        filters: Filters {
            include_excluded: true,
            ..Filters::default()
        },
        sort: Sort::ShapeDefault,
        after: None,
        limit: 200,
        fields: vec![Field::Name],
    }
}

pub fn search_options(limit: u32) -> SearchOptions {
    SearchOptions {
        after: None,
        limit,
        fields: vec![Field::Name],
        include_excluded: true,
        with_count: true,
    }
}

pub fn memo(company: CompanyId, at: i64) -> ActivityInput {
    ActivityInput {
        company,
        contact: None,
        kind: ActivityKind::Memo,
        at,
        note: Some(format!("synthetic-{at}")),
        status_after: None,
        next_action: NextAction::Keep,
        reverses: None,
    }
}

pub fn activities(db: &Engine, rows: i64) -> Vec<Activity> {
    let mut all = Vec::new();
    for id in 1..=rows {
        all.extend(
            db.company_detail(CompanyId(id), &vec![])
                .unwrap()
                .activities,
        );
    }
    all.sort_by_key(|a| a.id.0);
    all
}

pub fn verify(db: &Engine) {
    let report = db.verify().unwrap();
    assert!(
        report.integrity_ok && report.counts_match && report.folds_match,
        "{report:?}"
    );
}

pub fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").unwrap();
    out.flush().unwrap();
}

// パニック中も子を回収する。
pub struct ChildGuard {
    pub child: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
}

impl ChildGuard {
    pub fn spawn(test: &str, path: &Path, extra: &[(&str, &str)]) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(DB_ENV, path)
            .envs(extra.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.expect("child stdout read failed")).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            reader: Some(reader),
        }
    }

    pub fn receive(&self, prefix: &str) -> String {
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("child EOF or protocol timeout");
            // libtestは最初の行に試験名を付ける。
            if let Some((_, value)) = line.split_once(prefix) {
                return value.to_owned();
            }
        }
    }

    pub fn kill(&mut self) {
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        assert!(!status.success(), "child was not forcibly terminated");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
    }

    pub fn drain(&mut self) -> Vec<String> {
        self.reader.take().unwrap().join().unwrap();
        self.lines.try_iter().collect()
    }

    pub fn finish(&mut self) -> ExitStatus {
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "child exit timeout");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
