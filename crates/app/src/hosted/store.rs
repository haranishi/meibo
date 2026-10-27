use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::Duration,
};

use fs4::fs_std::FileExt;
use meibo_core::{
    Durability, Engine, OpenOptions,
    synth::{Profile, SynthConfig},
    types::DataOrigin,
};

use super::{
    HostedResult, Role, auth,
    control::ControlStore,
    sql::{Connection, OpenFlags, params},
};

const APPLICATION_ID: i64 = 0x6d62_6874;
pub(super) const DDL: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE users(id INTEGER PRIMARY KEY,username TEXT NOT NULL UNIQUE,password_hash TEXT NOT NULL,disabled INTEGER NOT NULL DEFAULT 0 CHECK(disabled IN(0,1)));
CREATE TABLE tenants(slug TEXT PRIMARY KEY);
CREATE TABLE memberships(user_id INTEGER NOT NULL REFERENCES users(id),tenant TEXT NOT NULL REFERENCES tenants(slug),role TEXT NOT NULL CHECK(role IN('viewer','editor','admin')),PRIMARY KEY(user_id,tenant));
CREATE TABLE sessions(digest BLOB PRIMARY KEY CHECK(length(digest)=32),user_id INTEGER NOT NULL,tenant TEXT NOT NULL,created_at INTEGER NOT NULL,last_seen INTEGER NOT NULL,expires_at INTEGER NOT NULL,FOREIGN KEY(user_id,tenant) REFERENCES memberships(user_id,tenant) ON DELETE CASCADE);
CREATE INDEX session_user ON sessions(user_id,tenant);
CREATE TABLE login_attempts(bucket TEXT PRIMARY KEY,window_start INTEGER NOT NULL,failures INTEGER NOT NULL);
CREATE TABLE audit(id INTEGER PRIMARY KEY,at INTEGER NOT NULL,actor TEXT NOT NULL,tenant TEXT,operation TEXT NOT NULL,outcome TEXT NOT NULL);
CREATE INDEX audit_tenant ON audit(tenant,id);
";

pub(super) struct Store {
    pub(super) root: PathBuf,
    control: ControlStore,
}

pub(super) struct MaintenanceGuard(File);
impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) fn engine_options() -> OpenOptions {
    OpenOptions {
        durability: Durability::Full,
        cache_mib: 32,
        background_checkpoint: false,
        ..OpenOptions::default()
    }
}

pub(super) fn valid_slug(value: &str) -> bool {
    (1..=48).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

pub(super) fn username(value: &str) -> HostedResult<String> {
    if !(1..=128).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
    {
        return Err("利用者名は128文字以内の英数字と @ . _ + - で指定してください。".into());
    }
    Ok(value.to_ascii_lowercase())
}

pub(super) fn regular_file(path: &Path) -> HostedResult<()> {
    let meta = path.symlink_metadata()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("通常の保存ファイルを指定してください。".into());
    }
    Ok(())
}

pub(super) fn directory(path: &Path) -> HostedResult<()> {
    let meta = path.symlink_metadata()?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("通常の専用ディレクトリを指定してください。".into());
    }
    Ok(())
}

pub(super) fn private_mode(path: &Path, directory: bool) -> HostedResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, directory);
    }
    Ok(())
}

fn configure(conn: &Connection) -> HostedResult<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    )?;
    Ok(())
}

impl Store {
    pub(super) fn initialize(root: &Path) -> HostedResult<Self> {
        if root.symlink_metadata().is_ok() {
            directory(root)?;
            if fs::read_dir(root)?.next().is_some() {
                return Err("保存先は空の専用ディレクトリを指定してください。".into());
            }
        } else {
            fs::create_dir(root)?;
        }
        private_mode(root, true)?;
        fs::create_dir(root.join("tenants"))?;
        private_mode(&root.join("tenants"), true)?;
        let path = root.join("control.sqlite3");
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        drop(file);
        private_mode(&path, false)?;
        let mut conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure(&conn)?;
        let tx = conn.transaction()?;
        tx.execute_batch(DDL)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", 1)?;
        let dummy = auth::hash_password(&auth::random_hex(32)?)?;
        for (key, value) in [("instance", auth::random_hex(16)?), ("dummy_hash", dummy)] {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)",
                params![key, value],
            )?;
        }
        tx.commit()?;
        let store = Self {
            root: fs::canonicalize(root)?,
            control: ControlStore { conn },
        };
        let _guard = store.lock()?;
        store.audit("maintenance", None, "initialize", "success")?;
        Ok(store)
    }

    pub(super) fn open(root: &Path) -> HostedResult<Self> {
        directory(root)?;
        directory(&root.join("tenants"))?;
        let path = root.join("control.sqlite3");
        regular_file(&path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if id != APPLICATION_ID || version != 1 {
            return Err("認証DBの保存形式を確認してください。".into());
        }
        configure(&conn)?;
        Ok(Self {
            root: fs::canonicalize(root)?,
            control: ControlStore { conn },
        })
    }

    pub(super) fn lock(&self) -> HostedResult<MaintenanceGuard> {
        let path = self.root.join(".maintenance.lock");
        if path.symlink_metadata().is_ok() {
            regular_file(&path)?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        private_mode(&path, false)?;
        if !FileExt::try_lock_exclusive(&file)? {
            return Err("保守処理中です。完了後にもう一度実行してください。".into());
        }
        Ok(MaintenanceGuard(file))
    }

    pub(super) fn tenant_path(&self, slug: &str) -> HostedResult<PathBuf> {
        if !valid_slug(slug) {
            return Err("組織IDが正しくありません。".into());
        }
        let found: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tenants WHERE slug=?1)",
            [slug],
            |r| r.get(0),
        )?;
        if !found {
            return Err("組織が見つかりません。".into());
        }
        directory(&self.root.join("tenants"))?;
        let path = self.root.join("tenants").join(format!("{slug}.sqlite3"));
        regular_file(&path)?;
        Ok(path)
    }

    pub(super) fn add_tenant(
        &mut self,
        slug: &str,
        origin: DataOrigin,
        demo_rows: u64,
    ) -> HostedResult<()> {
        if !valid_slug(slug)
            || demo_rows > 100_000
            || (origin != DataOrigin::Synthetic && demo_rows != 0)
        {
            return Err("組織ID・データ種別・合成件数を確認してください。".into());
        }
        let path = self.root.join("tenants").join(format!("{slug}.sqlite3"));
        let mut engine = Engine::create(&path, origin, &engine_options())?;
        private_mode(&path, false)?;
        if demo_rows > 0 {
            engine.load_synthetic(SynthConfig {
                rows: demo_rows,
                seed: 42,
                profile: Profile::Dense,
            })?;
        }
        self.conn
            .execute("INSERT INTO tenants(slug) VALUES(?1)", [slug])?;
        self.audit("maintenance", Some(slug), "tenant.add", "success")
    }

    pub(super) fn set_member(
        &mut self,
        name: &str,
        tenant: &str,
        role: Option<Role>,
    ) -> HostedResult<()> {
        username(name)?;
        self.tenant_path(tenant)?;
        self.control.set_member_record(name, tenant, role)
    }
}

impl std::ops::Deref for Store {
    type Target = ControlStore;
    fn deref(&self) -> &Self::Target {
        &self.control
    }
}

impl std::ops::DerefMut for Store {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.control
    }
}
