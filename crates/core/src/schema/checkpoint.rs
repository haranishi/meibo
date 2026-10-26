//! 確定後の通知だけで動くチェックポイント。

use super::{EngineError, OpenOptions, configure, connect, storage};
use crate::sql::Connection;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(test)]
pub(super) const WAL_THRESHOLD: u64 = 4 * 1024 * 1024;
const PENDING_FRAMES: i64 = 1000;
/// 1回の書き戻しの後の休み。止める合図の反応も、この間だけ遅れる。
const BATCH_PAUSE: Duration = Duration::from_millis(25);

pub(super) struct Checkpoint {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Checkpoint {
    pub(super) fn start(path: &Path, opt: &OpenOptions) -> Result<Option<Self>, EngineError> {
        if !opt.background_checkpoint {
            return Ok(None);
        }
        let worker_opt = OpenOptions {
            cache_mib: 8,
            ..opt.clone()
        };
        let conn = connect(path, &worker_opt)?;
        configure(&conn, &worker_opt)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("meibo-checkpoint".into())
            .spawn(move || run(&conn, &worker_stop))
            .map_err(storage)?;
        Ok(Some(Self {
            stop,
            worker: Some(worker),
        }))
    }

    pub(super) fn notify(&self, conn: &Connection) {
        if !state(conn).is_ok_and(|(log, done)| log - done >= PENDING_FRAMES) {
            return;
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
}

impl Drop for Checkpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn run(conn: &Connection, stop: &AtomicBool) {
    loop {
        thread::park();
        if stop.load(Ordering::Acquire) {
            break;
        }
        if state(conn).is_ok_and(|(log, done)| log - done >= PENDING_FRAMES) {
            // 読み取りや別のチェックポイントを待たず、次の通知に回す。
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
            // 書き込みが続く間、確定ごとに書き戻さない。休む間の通知は次の1回にまとまる。
            thread::sleep(BATCH_PAUSE);
        }
    }
}

/// NOOP は書き戻さず、現在の世代のフレーム数を返す。
pub(super) fn state(conn: &Connection) -> Result<(i64, i64), EngineError> {
    conn.query_row("PRAGMA wal_checkpoint(NOOP)", [], |r| {
        Ok((r.get(1)?, r.get(2)?))
    })
    .map_err(storage)
}

#[derive(Clone, Copy)]
pub(crate) enum Mode {
    Restart,
    Truncate,
}

/// 別のチェックポイントとの競合も、接続の待ち時間の中で再試行する。
pub(crate) fn retry(conn: &Connection, mode: Mode) -> Result<bool, EngineError> {
    let timeout: i64 = conn
        .pragma_query_value(None, "busy_timeout", |r| r.get(0))
        .map_err(storage)?;
    let timeout = u64::try_from(timeout).unwrap_or(0);
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let sql = match mode {
        Mode::Restart => "PRAGMA wal_checkpoint(RESTART)",
        Mode::Truncate => "PRAGMA wal_checkpoint(TRUNCATE)",
    };
    let result = (|| {
        loop {
            // SQLite 内部の待機も、残り時間に収める。
            let remaining = deadline.saturating_duration_since(Instant::now());
            conn.busy_timeout(remaining).map_err(storage)?;
            let busy: i64 = conn.query_row(sql, [], |r| r.get(0)).map_err(storage)?;
            if busy == 0 {
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    })();
    conn.busy_timeout(Duration::from_millis(timeout))
        .map_err(storage)?;
    result
}
