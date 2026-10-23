//! 公開口の呼び出し全体を計時する測定器。
use super::{
    Case, Measurer,
    conditions::{Operation, Prepared, query, search_options},
    percentiles,
    reader::Reader,
};
use meibo_core::{
    Durability, Engine, Integrity, OpenOptions,
    types::{
        ActivityInput, ActivityKind, BulkChange, CompanyId, Filters, NextAction, OwnerChange,
        Status, UserId,
    },
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

type Result<T> = std::result::Result<T, String>;

pub(super) struct EngineMeasurer {
    pub engine: Engine,
    pub prepared: Vec<Prepared>,
    pub extra: BTreeMap<String, String>,
    activity_ids: Vec<CompanyId>,
    activity_index: usize,
    list_limit: u32,
}
impl EngineMeasurer {
    pub(super) fn new(engine: Engine, prepared: Vec<Prepared>) -> Result<Self> {
        let mut q = query(Filters::default());
        q.limit = 200;
        let mut activity_ids = Vec::new();
        while activity_ids.len() < 303 {
            let page = engine.list(&q).map_err(|e| e.to_string())?;
            activity_ids.extend(page.rows.into_iter().map(|c| c.id));
            let Some(next) = page.next else {
                break;
            };
            q.after = Some(next);
        }
        let n = engine
            .count(&Filters::default())
            .map_err(|e| e.to_string())?
            .n
            .min(100_000);
        if n == 0 {
            return Err("活動・リストの対象企業がありません".into());
        }
        let list_limit = u32::try_from(n).map_err(|e| e.to_string())?;
        let mut extra = BTreeMap::new();
        extra.insert("list_companies".into(), n.to_string());
        Ok(Self {
            engine,
            prepared,
            extra,
            activity_ids,
            activity_index: 0,
            list_limit,
        })
    }
    fn activity_samples(&mut self, trials: usize) -> Result<Vec<f64>> {
        let mut samples = Vec::new();
        for i in 0..=trials {
            let company = *self
                .activity_ids
                .get(self.activity_index)
                .ok_or("別々の企業への活動に必要な社数が不足しています")?;
            self.activity_index += 1;
            let a = ActivityInput {
                company,
                contact: None,
                kind: [
                    ActivityKind::Call,
                    ActivityKind::Visit,
                    ActivityKind::Email,
                    ActivityKind::Memo,
                ][i % 4],
                at: 1_792_627_200,
                note: None,
                status_after: if i % 2 == 0 {
                    Some(Status::InProgress)
                } else {
                    None
                },
                next_action: match i % 3 {
                    0 => NextAction::Set(1_800_000_000),
                    1 => NextAction::Clear,
                    _ => NextAction::Keep,
                },
                reverses: None,
            };
            let start = Instant::now();
            self.engine.record_activity(&a).map_err(|e| e.to_string())?;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if i > 0 {
                samples.push(elapsed);
            }
        }
        Ok(samples)
    }
    fn create_samples(&mut self, trials: usize) -> Result<Vec<f64>> {
        let f = Filters::default();
        let mut samples = Vec::new();
        for i in 0..=trials {
            let start = Instant::now();
            let list = self
                .engine
                .create_list("bench", &f, self.list_limit)
                .map_err(|e| e.to_string())?;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            self.engine.delete_list(list).map_err(|e| e.to_string())?;
            if i > 0 {
                samples.push(elapsed);
            }
        }
        Ok(samples)
    }
    fn bulk_samples(&mut self, trials: usize) -> Result<Vec<f64>> {
        let list = self
            .engine
            .create_list("bench-bulk", &Filters::default(), self.list_limit)
            .map_err(|e| e.to_string())?;
        let result = (|| {
            self.engine
                .bulk_update(
                    list,
                    &BulkChange {
                        owner: OwnerChange::Keep,
                        status: Some(Status::Untouched),
                        include_excluded: false,
                    },
                )
                .map_err(|e| e.to_string())?;
            self.extra.insert(
                "bulk_update_settle".into(),
                "各回の前に、前の回の書き戻しの完了を待つ（単独の一括変更の時間）".into(),
            );
            let mut samples = Vec::new();
            // 保存先が止まると書き戻しの待ちが延びる。時間に入らない待ちも記録に残す。
            let mut settle_max_ms = 0.0_f64;
            let mut settle_timeouts = 0_u32;
            for i in 0..=trials {
                let change = BulkChange {
                    owner: OwnerChange::Set(UserId(if i % 2 == 0 { 1 } else { 2 })),
                    status: Some(if i % 2 == 0 {
                        Status::InProgress
                    } else {
                        Status::Won
                    }),
                    include_excluded: false,
                };
                let waited = Instant::now();
                if !self.engine.settle().map_err(|e| e.to_string())? {
                    settle_timeouts += 1;
                }
                settle_max_ms = settle_max_ms.max(waited.elapsed().as_secs_f64() * 1000.0);
                let start = Instant::now();
                let n = self
                    .engine
                    .bulk_update(list, &change)
                    .map_err(|e| e.to_string())?;
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if n != u64::from(self.list_limit) {
                    return Err(format!("一括変更の社数不一致: {n}"));
                }
                if i > 0 {
                    samples.push(elapsed);
                }
            }
            self.extra.insert(
                "bulk_update_settle_max_ms".into(),
                settle_max_ms.to_string(),
            );
            self.extra.insert(
                "bulk_update_settle_timeouts".into(),
                settle_timeouts.to_string(),
            );
            self.bulk_back_to_back(list, trials + 1)?;
            Ok(samples)
        })();
        let deleted = self.engine.delete_list(list).map_err(|e| e.to_string());
        result.and_then(|samples| deleted.map(|()| samples))
    }
    fn bulk_back_to_back(&mut self, list: meibo_core::types::ListId, offset: usize) -> Result<()> {
        let mut samples = Vec::new();
        let path = self.engine.path().ok_or("DBパスなし")?.to_path_buf();
        let mut wal_bytes = sizes(&path).1;
        for i in offset..offset + 10 {
            let change = BulkChange {
                owner: OwnerChange::Set(UserId(if i % 2 == 0 { 1 } else { 2 })),
                status: Some(if i % 2 == 0 {
                    Status::InProgress
                } else {
                    Status::Won
                }),
                include_excluded: false,
            };
            let start = Instant::now();
            let n = self
                .engine
                .bulk_update(list, &change)
                .map_err(|e| e.to_string())?;
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            if n != u64::from(self.list_limit) {
                return Err(format!("一括変更の社数不一致: {n}"));
            }
            wal_bytes = wal_bytes.max(sizes(&path).1);
        }
        // 参考用の10標本では nearest-rank のp95は最大値になる。
        samples.sort_by(f64::total_cmp);
        self.extra.insert(
            "bulk_update_back_to_back_p95_ms".into(),
            samples[(95 * samples.len()).div_ceil(100) - 1].to_string(),
        );
        self.extra.insert(
            "bulk_update_back_to_back_wal_bytes".into(),
            wal_bytes.to_string(),
        );
        Ok(())
    }
    pub(super) fn full_reference(&mut self) -> Result<()> {
        let path = self.engine.path().ok_or("DBパスなし")?.to_path_buf();
        let full = Engine::open(
            &path,
            &OpenOptions {
                durability: Durability::Full,
                integrity: Integrity::Never,
                ..OpenOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let normal = std::mem::replace(&mut self.engine, full);
        let result = self.activity_samples(100);
        self.engine = normal;
        let samples = result?;
        self.extra.insert(
            "activity_full_p95_ms".into(),
            percentiles(&samples)?.1.to_string(),
        );
        self.extra.insert(
            "activity_full_samples".into(),
            serde_json::to_string(&samples).map_err(|e| e.to_string())?,
        );
        Ok(())
    }
}
impl Measurer for EngineMeasurer {
    fn measure(&mut self, case: &Case) -> Result<Vec<f64>> {
        let operation = self
            .prepared
            .iter()
            .find(|p| p.case.id == case.id)
            .ok_or("条件なし")?
            .operation
            .clone();
        match operation {
            Operation::Activity(with_reader) => {
                let reader = if with_reader {
                    Some(Reader::start(self.engine.path().ok_or("DBパスなし")?)?)
                } else {
                    None
                };
                let samples = self.activity_samples(case.trials);
                if !with_reader && samples.is_ok() {
                    self.full_reference()?;
                }
                if let Some(reader) = reader {
                    let n = reader.finish()?;
                    self.extra.insert("reader_iterations".into(), n.to_string());
                    self.extra
                        .insert("reader_process_used".into(), "true".into());
                }
                samples
            }
            Operation::CreateList => self.create_samples(case.trials),
            Operation::BulkUpdate => self.bulk_samples(case.trials),
            other => {
                let mut samples = Vec::new();
                for i in 0..=case.trials {
                    let elapsed = match &other {
                        Operation::List(q) => {
                            let start = Instant::now();
                            let value = self.engine.list(q).map_err(|e| e.to_string())?;
                            let elapsed = start.elapsed();
                            std::hint::black_box(value);
                            elapsed
                        }
                        Operation::Count(f) => {
                            let start = Instant::now();
                            let value = self.engine.count(f).map_err(|e| e.to_string())?;
                            let elapsed = start.elapsed();
                            std::hint::black_box(value);
                            elapsed
                        }
                        Operation::Search(term, count) => {
                            let opt = search_options(*count);
                            let start = Instant::now();
                            let value = self
                                .engine
                                .search_name(term, &opt)
                                .map_err(|e| e.to_string())?;
                            let elapsed = start.elapsed();
                            std::hint::black_box(value);
                            elapsed
                        }
                        _ => unreachable!(),
                    };
                    if i > 0 {
                        samples.push(elapsed.as_secs_f64() * 1000.0);
                    }
                }
                Ok(samples)
            }
        }
    }
}

pub(super) struct DatabaseFiles {
    pub directory: PathBuf,
    pub keep: bool,
}
impl Drop for DatabaseFiles {
    fn drop(&mut self) {
        if !self.keep {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(self.directory.join(format!("engine.db{suffix}")));
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }
}
pub(super) fn sizes(path: &Path) -> (u64, u64) {
    let size = |p: &Path| fs::metadata(p).map_or(0, |m| m.len());
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    (size(path), size(Path::new(&wal)))
}
