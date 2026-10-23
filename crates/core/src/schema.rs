//! SQLite の保存形式、接続設定、移行と保全。

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::types::{DataOrigin, EngineError};

pub(crate) mod checkpoint;
pub(crate) mod counts;
pub(crate) mod fts;
mod migration;
pub(crate) use migration::{MIGRATIONS, migrate_with};

pub(crate) struct Migration {
    pub(crate) from: u32,
    pub(crate) to: u32,
    pub(crate) apply: fn(&Transaction<'_>) -> Result<(), EngineError>,
}

#[cfg(test)]
mod tests;

pub(crate) const VERSION: u32 = 1;
// 1回の確定で増える主ファイルとWALの余裕。
pub(crate) const WRITE_BASE: u64 = 1024 * 1024;

pub(crate) const SECONDARY_INDEXES: [(&str, &str); 10] = [
    (
        "k1",
        "CREATE INDEX k1 ON companies(prefecture_code, industry_code, status, has_phone, id);",
    ),
    (
        "k2",
        "CREATE INDEX k2 ON companies(prefecture_code, status, has_phone, id);",
    ),
    (
        "k3",
        "CREATE INDEX k3 ON companies(municipality_code, industry_code, status, has_phone, id);",
    ),
    (
        "k4",
        "CREATE INDEX k4 ON companies(municipality_code, status, has_phone, id);",
    ),
    (
        "k5",
        "CREATE INDEX k5 ON companies(industry_code, status, has_phone, id);",
    ),
    ("k6", "CREATE INDEX k6 ON companies(status, has_phone, id);"),
    ("k7", "CREATE INDEX k7 ON companies(has_phone, status, id);"),
    (
        "k8",
        "CREATE INDEX k8 ON companies(owner_id, next_action_at, status, id) WHERE owner_id IS NOT NULL;",
    ),
    (
        "k9",
        "CREATE INDEX k9 ON companies(next_action_at, status, id) WHERE next_action_at IS NOT NULL;",
    ),
    (
        "k10",
        "CREATE INDEX k10 ON companies(last_activity_at, status, id);",
    ),
];

const APPLICATION_ID: u32 = 0x6D65_6962;

/// 確定時の耐久性。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Durability {
    /// WAL の同期をチェックポイント時に行う。
    #[default]
    Normal,
    /// 確定ごとに WAL を同期する。
    Full,
}

/// 開くときの簡易整合性検査。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Integrity {
    /// 空でない WAL が残っているときに検査する。
    #[default]
    Auto,
    /// 常に検査する。
    Always,
    /// 起動時には検査しない。
    Never,
}

/// 空き容量の問い合わせ。試験では合成の空き容量を注入できる。
pub trait SpaceProbe: Send + Sync {
    /// 指定したパスのファイルシステムの空きバイト数。
    ///
    /// # Errors
    /// ファイルシステムを問い合わせられなかった場合。
    fn available(&self, path: &Path) -> io::Result<u64>;
}

struct FilesystemSpace;
impl SpaceProbe for FilesystemSpace {
    fn available(&self, path: &Path) -> io::Result<u64> {
        fs4::available_space(path)
    }
}

/// 接続ごとの設定。
#[derive(Clone)]
pub struct OpenOptions {
    /// 確定時の耐久性。既定は NORMAL。
    pub durability: Durability,
    /// 接続のキャッシュ上限（MiB）。既定は512、最大2,097,151。
    pub cache_mib: u32,
    /// 専用スレッドでチェックポイントする。既定は true。
    pub background_checkpoint: bool,
    /// WAL の背圧をかける上限（MiB）。既定は512、最小8。1回の書き込みで超えうる。
    /// 見積りが上限の1/32未満なら背圧を省き、軽い書き込みを待たせない（要件5.5・5.6）。
    /// WALの歯止めは重い書き込みの背圧と、書き込み前の空き容量確認。
    /// 他アプリの読み取り中は巻き戻せないため、`busy_timeout` 後は書き込みを進める。
    pub wal_limit_mib: u32,
    /// ロック待ち時間（ミリ秒）。既定は5000、最大2,147,483,647。
    pub busy_timeout_ms: u32,
    /// 開くときの整合性検査。既定は Auto。
    pub integrity: Integrity,
    /// 未指定ならファイルシステムへ問い合わせる。
    pub space_probe: Option<Arc<dyn SpaceProbe>>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            durability: Durability::Normal,
            cache_mib: 512,
            background_checkpoint: true,
            wal_limit_mib: 512,
            busy_timeout_ms: 5000,
            integrity: Integrity::Auto,
            space_probe: None,
        }
    }
}

pub(crate) struct Db {
    conn: Connection,
    checkpoint: Option<checkpoint::Checkpoint>,
    origin: DataOrigin,
    opt: OpenOptions,
}

impl Db {
    pub(crate) fn create(
        path: &Path,
        origin: DataOrigin,
        opt: &OpenOptions,
    ) -> Result<Self, EngineError> {
        validate_options(opt)?;
        ensure_space(path, 4 * WRITE_BASE, opt.space_probe.as_deref())?;
        // 既存の保存先を上書きしない。
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(storage)?;
        drop(file);
        let result = (|| {
            let mut conn = connect(path, opt)?;
            configure(&conn, opt)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            tx.execute_batch(include_str!("schema/ddl.sql"))
                .map_err(storage)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(storage)?;
            tx.pragma_update(None, "user_version", VERSION)
                .map_err(storage)?;
            for (key, value) in [
                ("schema_version", VERSION.to_string()),
                ("data_origin", origin_text(origin).to_owned()),
                ("created_at", unix_seconds()?.to_string()),
            ] {
                tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2)", (key, value))
                    .map_err(storage)?;
            }
            counts::recount(&tx)?;
            tx.commit().map_err(storage)?;
            let checkpoint = checkpoint::Checkpoint::start(path, opt)?;
            Ok(Self {
                conn,
                checkpoint,
                origin,
                opt: opt.clone(),
            })
        })();
        if result.is_err() {
            fs::remove_file(path).map_err(storage)?;
        }
        result
    }

    pub(crate) fn open(path: &Path, opt: &OpenOptions) -> Result<Self, EngineError> {
        let check = should_check(path, opt.integrity)?;
        let conn = connect(path, opt)?;
        validate_header(&conn)?;
        check_version(version(&conn)?, VERSION)?;
        if check {
            check_integrity(&conn, true)?;
        }
        let origin = read_origin(&conn)?;
        configure(&conn, opt)?;
        let checkpoint = checkpoint::Checkpoint::start(path, opt)?;
        Ok(Self {
            conn,
            checkpoint,
            origin,
            opt: opt.clone(),
        })
    }

    pub(crate) fn space_probe(&self) -> Option<&dyn SpaceProbe> {
        self.opt.space_probe.as_deref()
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn write<T>(
        &mut self,
        estimate: u64,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let path = self.conn.path().ok_or_else(|| EngineError::InvalidInput {
            field: "path".into(),
            reason: "file-backed database required".into(),
        })?;
        ensure_space(Path::new(path), estimate, self.space_probe())?;
        self.apply_backpressure(estimate)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        match f(&tx) {
            Ok(value) => {
                tx.commit().map_err(storage)?;
                if let Some(checkpoint) = &self.checkpoint {
                    checkpoint.notify(&self.conn);
                }
                Ok(value)
            }
            Err(err) => {
                tx.rollback().map_err(storage)?;
                Err(err)
            }
        }
    }

    fn apply_backpressure(&self, estimate: u64) -> Result<(), EngineError> {
        let limit_bytes = u64::from(self.opt.wal_limit_mib) * 1024 * 1024;
        // 軽い書き込みは読み取り解放を待たせない（要件5.5・5.6）。
        if self.checkpoint.is_none() || estimate < limit_bytes / 32 {
            return Ok(());
        }
        let (log, _) = checkpoint::state(&self.conn)?;
        let page_size: u64 = self
            .conn
            .pragma_query_value(None, "page_size", unsigned)
            .map_err(storage)?;
        let limit = limit_bytes / page_size;
        if u64::try_from(log).unwrap_or(0) >= limit {
            // 他アプリの読み取り中は巻き戻せず、待っても進まない。
            // 重い書き込みも期限後は進め、空き容量確認を歯止めにする。
            self.settle()?;
        }
        Ok(())
    }

    /// 完了または背景スレッドなしなら `true`、`busy_timeout` の期限切れなら `false`。
    pub(crate) fn settle(&self) -> Result<bool, EngineError> {
        if self.checkpoint.is_some() {
            return checkpoint::retry(&self.conn, checkpoint::Mode::Restart);
        }
        Ok(true)
    }

    pub(crate) const fn origin(&self) -> DataOrigin {
        self.origin
    }

    pub(crate) fn check_origin(&self, claim: DataOrigin) -> Result<(), EngineError> {
        if claim == self.origin {
            Ok(())
        } else {
            Err(EngineError::OriginMismatch)
        }
    }

    pub(crate) fn verify_integrity(&self) -> Result<(), EngineError> {
        check_integrity(&self.conn, false)
    }

    pub(crate) fn backup_to(&self, dest: &Path) -> Result<(), EngineError> {
        ensure_space(
            dest,
            space_needed(&self.conn)?,
            self.opt.space_probe.as_deref(),
        )?;
        backup(&self.conn, dest)
    }

    pub(crate) fn migrate(path: &Path, opt: &OpenOptions) -> Result<(), EngineError> {
        migrate_with(path, MIGRATIONS, opt.space_probe.as_deref(), opt)
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        // 主接続の最終チェックポイントと競合させない。
        drop(self.checkpoint.take());
    }
}

pub(crate) fn with_erase_gate<T>(
    tx: &Transaction<'_>,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    // 入れ子でも外側のゲートを消さない。
    tx.execute("INSERT INTO erase_gate(x) VALUES(1)", [])
        .map_err(storage)?;
    let row = tx.last_insert_rowid();
    let result = f(tx);
    tx.execute("DELETE FROM erase_gate WHERE rowid=?1", [row])
        .map_err(storage)?;
    result
}

pub(crate) fn ensure_space(
    path: &Path,
    needed_bytes: u64,
    probe: Option<&dyn SpaceProbe>,
) -> Result<(), EngineError> {
    let mut existing = path;
    while !existing.exists() {
        existing = existing
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
    }
    let available = probe
        .unwrap_or(&FilesystemSpace)
        .available(existing)
        .map_err(storage)?;
    if available < needed_bytes {
        Err(EngineError::InsufficientSpace)
    } else {
        Ok(())
    }
}

fn storage(err: impl std::error::Error + Send + Sync + 'static) -> EngineError {
    EngineError::Storage(Box::new(err))
}

fn database_error(err: rusqlite::Error) -> EngineError {
    match err.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            EngineError::Corrupt
        }
        _ => storage(err),
    }
}

fn connect(path: &Path, opt: &OpenOptions) -> Result<Connection, EngineError> {
    validate_options(opt)?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(database_error)?;
    conn.busy_timeout(Duration::from_millis(u64::from(opt.busy_timeout_ms)))
        .map_err(storage)?;
    Ok(conn)
}

fn configure(conn: &Connection, opt: &OpenOptions) -> Result<(), EngineError> {
    let mode: String = conn
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .map_err(database_error)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(storage(io::Error::other("WAL unavailable")));
    }
    conn.pragma_update(
        None,
        "synchronous",
        match opt.durability {
            Durability::Normal => "NORMAL",
            Durability::Full => "FULL",
        },
    )
    .map_err(storage)?;
    if opt.durability == Durability::Full {
        conn.pragma_update(None, "fullfsync", true)
            .map_err(storage)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true)
            .map_err(storage)?;
    }
    conn.pragma_update(
        None,
        "wal_autocheckpoint",
        if opt.background_checkpoint { 0 } else { 1000 },
    )
    .map_err(storage)?;
    conn.pragma_update(None, "journal_size_limit", 64 * 1024 * 1024)
        .map_err(storage)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(storage)?;
    conn.pragma_update(None, "cache_size", -(i64::from(opt.cache_mib) * 1024))
        .map_err(storage)?;
    Ok(())
}

fn validate_header(conn: &Connection) -> Result<(), EngineError> {
    let id: u32 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(database_error)?;
    if id != APPLICATION_ID {
        return Err(EngineError::Corrupt);
    }
    Ok(())
}

fn version(conn: &Connection) -> Result<u32, EngineError> {
    let value: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| EngineError::Corrupt)?;
    value.parse().map_err(|_| EngineError::Corrupt)
}

fn check_version(found: u32, supported: u32) -> Result<(), EngineError> {
    match found.cmp(&supported) {
        std::cmp::Ordering::Less => Err(EngineError::NeedsMigration),
        std::cmp::Ordering::Greater => Err(EngineError::UnsupportedVersion { found, supported }),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn read_origin(conn: &Connection) -> Result<DataOrigin, EngineError> {
    let text: String = conn
        .query_row("SELECT value FROM meta WHERE key='data_origin'", [], |r| {
            r.get(0)
        })
        .map_err(|_| EngineError::Corrupt)?;
    match text.as_str() {
        "synthetic" => Ok(DataOrigin::Synthetic),
        "real" => Ok(DataOrigin::Real),
        _ => Err(EngineError::Corrupt),
    }
}

const fn origin_text(origin: DataOrigin) -> &'static str {
    match origin {
        DataOrigin::Synthetic => "synthetic",
        DataOrigin::Real => "real",
    }
}

fn should_check(path: &Path, integrity: Integrity) -> Result<bool, EngineError> {
    match integrity {
        Integrity::Always => Ok(true),
        Integrity::Never => Ok(false),
        Integrity::Auto => match fs::metadata(suffixed(path, "-wal")) {
            Ok(meta) => Ok(meta.len() > 0),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(storage(e)),
        },
    }
}

fn check_integrity(conn: &Connection, quick: bool) -> Result<(), EngineError> {
    let mut stmt = conn
        .prepare(if quick {
            "PRAGMA quick_check(1)"
        } else {
            "PRAGMA integrity_check"
        })
        .map_err(database_error)?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(database_error)?;
    let mut ok = false;
    for row in rows {
        if row.map_err(database_error)? != "ok" {
            return Err(EngineError::Corrupt);
        }
        ok = true;
    }
    if ok {
        Ok(())
    } else {
        Err(EngineError::Corrupt)
    }
}

fn space_needed(conn: &Connection) -> Result<u64, EngineError> {
    let pages: u64 = conn
        .pragma_query_value(None, "page_count", unsigned)
        .map_err(database_error)?;
    let page_size: u64 = conn
        .pragma_query_value(None, "page_size", unsigned)
        .map_err(database_error)?;
    Ok(pages
        .saturating_mul(page_size)
        .saturating_mul(6)
        .div_ceil(5)
        .saturating_add(64 * 1024 * 1024))
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn unix_seconds() -> Result<u64, EngineError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(storage)?
        .as_secs())
}

fn backup(conn: &Connection, dest: &Path) -> Result<(), EngineError> {
    drop(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .map_err(storage)?,
    );
    let result = conn
        .backup(rusqlite::MAIN_DB, dest, None)
        .map_err(database_error);
    if result.is_err() {
        fs::remove_file(dest).map_err(storage)?;
    }
    result
}

fn unsigned(row: &rusqlite::Row<'_>) -> rusqlite::Result<u64> {
    let value: i64 = row.get(0)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn validate_options(opt: &OpenOptions) -> Result<(), EngineError> {
    if opt.wal_limit_mib < 8 {
        return Err(EngineError::InvalidInput {
            field: "wal_limit_mib".into(),
            reason: "must be at least 8 MiB".into(),
        });
    }
    for (field, value, limit) in [
        ("cache_mib", opt.cache_mib, i32::MAX.unsigned_abs() / 1024),
        (
            "busy_timeout_ms",
            opt.busy_timeout_ms,
            i32::MAX.unsigned_abs(),
        ),
    ] {
        if value > limit {
            return Err(EngineError::InvalidInput {
                field: field.to_owned(),
                reason: format!("must be at most {limit} for SQLite"),
            });
        }
    }
    Ok(())
}
