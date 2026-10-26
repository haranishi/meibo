use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::Path,
};

use meibo_core::Engine;
use rusqlite::{Connection, MAIN_DB, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    HostedResult, Store, auth,
    store::{directory, engine_options, private_mode, regular_file, valid_slug},
};

fn file_hash(path: &Path) -> HostedResult<String> {
    regular_file(path)?;
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 65_536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(auth::hex(&hash.finalize()))
}

fn tenant_names(conn: &Connection) -> HostedResult<Vec<String>> {
    Ok(conn
        .prepare("SELECT slug FROM tenants ORDER BY slug")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}

fn verify_control(path: &Path) -> HostedResult<()> {
    regular_file(path)?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let result: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    let foreign: bool = conn.prepare("PRAGMA foreign_key_check")?.exists([])?;
    let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if result != "ok" || foreign || id != 0x6d62_6874 || version != 1 {
        return Err("認証DBの整合性を確認できません。".into());
    }
    Ok(())
}

fn verify_tenant(path: &Path) -> HostedResult<()> {
    let engine = Engine::open(path, &engine_options())?;
    let report = engine.verify()?;
    if !report.integrity_ok || !report.counts_match || !report.folds_match {
        return Err("組織DBの整合性を確認できません。".into());
    }
    Ok(())
}

pub(super) fn create(store: &Store, destination: &Path) -> HostedResult<()> {
    let _guard = store.lock()?;
    if destination.symlink_metadata().is_ok() {
        return Err("backup先は未存在のディレクトリを指定してください。".into());
    }
    let tenants = tenant_names(&store.conn)?;
    if tenants.len() > 1000 {
        return Err("1回のbackupは1000組織までです。".into());
    }
    fs::create_dir(destination)?;
    private_mode(destination, true)?;
    fs::create_dir(destination.join("tenants"))?;
    private_mode(&destination.join("tenants"), true)?;
    let control = destination.join("control.sqlite3");
    store.conn.backup(MAIN_DB, &control, None)?;
    private_mode(&control, false)?;
    {
        let conn = Connection::open_with_flags(&control, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; DELETE FROM sessions;")?;
    }
    verify_control(&control)?;
    let mut files = vec![json!({"name":"control.sqlite3","sha256":file_hash(&control)?})];
    for tenant in &tenants {
        let source = store.tenant_path(tenant)?;
        let name = format!("tenants/{tenant}.sqlite3");
        let target = destination.join(&name);
        Engine::open(&source, &engine_options())?.backup_to(&target)?;
        private_mode(&target, false)?;
        verify_tenant(&target)?;
        files.push(json!({"name":name,"sha256":file_hash(&target)?}));
    }
    let manifest =
        serde_json::to_vec_pretty(&json!({"version":1,"tenants":tenants,"files":files}))?;
    if manifest.len() > 65536 {
        return Err("backup目録の上限を超えています。保存先は不完全です。".into());
    }
    let path = destination.join("manifest.json");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    private_mode(&path, false)?;
    std::io::Write::write_all(&mut output, &manifest)?;
    output.sync_all()?;
    store.audit("maintenance", None, "backup", "success")?;
    Ok(())
}

pub(super) fn restore(source: &Path, destination: &Path) -> HostedResult<()> {
    if destination.symlink_metadata().is_ok() {
        return Err("restore先は未存在の専用ROOTを指定してください。".into());
    }
    directory(source)?;
    directory(&source.join("tenants"))?;
    let path = source.join("manifest.json");
    regular_file(&path)?;
    if path.metadata()?.len() > 65536 {
        return Err("backup目録の上限を超えています。".into());
    }
    let manifest: Value = serde_json::from_reader(File::open(path)?)?;
    if manifest.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("backupの版を確認してください。".into());
    }
    let tenants = manifest
        .get("tenants")
        .and_then(Value::as_array)
        .ok_or("組織の目録がありません。")?;
    if tenants.len() > 1000 {
        return Err("組織数の上限を超えています。".into());
    }
    let mut names = BTreeSet::from(["control.sqlite3".to_owned()]);
    let mut slugs = BTreeSet::new();
    for tenant in tenants {
        let slug = tenant
            .as_str()
            .filter(|s| valid_slug(s))
            .ok_or("組織の目録が正しくありません。")?;
        if !slugs.insert(slug.to_owned()) {
            return Err("組織の目録が重複しています。".into());
        }
        names.insert(format!("tenants/{slug}.sqlite3"));
    }
    let entries = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or("保存ファイルの目録がありません。")?;
    if entries.len() != names.len() {
        return Err("保存ファイルの目録が一致しません。".into());
    }
    let mut checked = BTreeSet::new();
    let mut bytes = 0_u64;
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| names.contains(*n))
            .ok_or("保存ファイル名が正しくありません。")?;
        if !checked.insert(name.to_owned()) {
            return Err("保存ファイルが重複しています。".into());
        }
        let path = source.join(name);
        if entry.get("sha256").and_then(Value::as_str) != Some(file_hash(&path)?.as_str()) {
            return Err("backupのhashが一致しません。".into());
        }
        bytes = bytes.saturating_add(path.metadata()?.len());
    }
    verify_control(&source.join("control.sqlite3"))?;
    {
        let conn = Connection::open_with_flags(
            source.join("control.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        if tenant_names(&conn)?.into_iter().collect::<BTreeSet<_>>() != slugs {
            return Err("認証DBと組織目録が一致しません。".into());
        }
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if fs4::available_space(parent)? < bytes.saturating_mul(2).saturating_add(32 * 1024 * 1024) {
        return Err("復旧先の空き容量が不足しています。".into());
    }
    fs::create_dir(destination)?;
    private_mode(destination, true)?;
    fs::create_dir(destination.join("tenants"))?;
    private_mode(&destination.join("tenants"), true)?;
    for name in names {
        fs::copy(source.join(&name), destination.join(&name))?;
        private_mode(&destination.join(name), false)?;
    }
    let restored = Store::open(destination)?;
    restored.conn.execute("DELETE FROM sessions", [])?;
    verify_control(&destination.join("control.sqlite3"))?;
    for slug in slugs {
        verify_tenant(&restored.tenant_path(&slug)?)?;
    }
    restored.audit("maintenance", None, "restore", "success")?;
    Ok(())
}
