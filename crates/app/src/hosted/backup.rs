use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{ErrorKind, Read},
    path::Path,
};

use super::sql::{Connection, MAIN_DB, OpenFlags};
use meibo_core::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    HostedResult, Store, auth,
    store::{directory, engine_options, private_mode, regular_file, valid_slug},
};

const MAX_MANIFEST_BYTES: usize = 64 * 1024;

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

pub(super) fn create(store: &Store, destination: &Path) -> HostedResult<String> {
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
    store
        .conn
        .backup(MAIN_DB, &control, None::<fn(super::sql::backup::Progress)>)?;
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
    if manifest.len() > MAX_MANIFEST_BYTES {
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
    Ok(auth::hex(&Sha256::digest(&manifest)))
}

fn require_new_root(destination: &Path) -> HostedResult<()> {
    match destination.symlink_metadata() {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("restore先は未存在の専用ROOTを指定してください。".into()),
    }
}

fn read_manifest(source: &Path, fingerprint: &str) -> HostedResult<Value> {
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("--manifest-sha256には64桁の16進数を指定してください。".into());
    }
    directory(source)?;
    let path = source.join("manifest.json");
    regular_file(&path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("backup目録の上限を超えています。".into());
    }
    if !auth::hex(&Sha256::digest(&bytes)).eq_ignore_ascii_case(fingerprint) {
        return Err("backup目録の指紋が一致しません。".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn verify_file_hash(path: &Path, expected: &str) -> HostedResult<()> {
    if file_hash(path)? != expected {
        return Err("backupのhashが一致しません。".into());
    }
    Ok(())
}

pub(super) fn restore(source: &Path, destination: &Path, fingerprint: &str) -> HostedResult<()> {
    require_new_root(destination)?;
    let manifest = read_manifest(source, fingerprint)?;
    directory(&source.join("tenants"))?;
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
    let mut hashes = BTreeMap::new();
    let mut bytes = 0_u64;
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| names.contains(*n))
            .ok_or("保存ファイル名が正しくありません。")?;
        let expected = entry
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or("保存ファイルのhashがありません。")?;
        if hashes.insert(name, expected).is_some() {
            return Err("保存ファイルが重複しています。".into());
        }
        let path = source.join(name);
        verify_file_hash(&path, expected)?;
        bytes = bytes.saturating_add(path.metadata()?.len());
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if parent.canonicalize()?.starts_with(source.canonicalize()?) {
        return Err("復元先は保存物の外に指定してください。".into());
    }
    if fs4::available_space(parent)? < bytes.saturating_mul(2).saturating_add(32 * 1024 * 1024) {
        return Err("復旧先の空き容量が不足しています。".into());
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix(".meibo-restore-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let staging = builder.tempdir_in(parent)?;
    private_mode(staging.path(), true)?;
    restore_databases(source, staging.path(), &hashes, &slugs)?;
    // SQLite has closed its connections and checkpointed the final session and audit changes.
    for name in hashes.keys() {
        File::options()
            .write(true)
            .open(staging.path().join(name))?
            .sync_all()?;
    }
    #[cfg(unix)]
    {
        File::open(staging.path().join("tenants"))?.sync_all()?;
        File::open(staging.path())?.sync_all()?;
    }
    require_new_root(destination)?;
    fs::rename(staging.path(), destination)?;
    let _ = staging.keep();
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn restore_databases(
    source: &Path,
    staging: &Path,
    hashes: &BTreeMap<&str, &str>,
    slugs: &BTreeSet<String>,
) -> HostedResult<()> {
    let tenants = staging.join("tenants");
    fs::create_dir(&tenants)?;
    private_mode(&tenants, true)?;
    for (&name, &expected) in hashes {
        let from = source.join(name);
        let to = staging.join(name);
        regular_file(&from)?;
        fs::copy(from, &to)?;
        private_mode(&to, false)?;
        verify_file_hash(&to, expected)?;
    }
    let control = staging.join("control.sqlite3");
    verify_control(&control)?;
    let restored = Store::open(staging)?;
    let control_slugs: BTreeSet<_> = tenant_names(&restored.conn)?.into_iter().collect();
    if &control_slugs != slugs {
        return Err("認証DBと組織目録が一致しません。".into());
    }
    restored.conn.execute("DELETE FROM sessions", [])?;
    for slug in slugs {
        verify_tenant(&restored.tenant_path(slug)?)?;
    }
    restored.audit("maintenance", None, "restore", "success")?;
    verify_control(&control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_copy_must_match_the_hash_checked_before_copying() {
        let source = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let name = "control.sqlite3";
        let path = source.path().join(name);
        fs::write(&path, b"first fixture").unwrap();
        let expected = file_hash(&path).unwrap();
        fs::write(&path, b"replacement fixture").unwrap();
        let hashes = BTreeMap::from([(name, expected.as_str())]);
        let error = restore_databases(source.path(), staging.path(), &hashes, &BTreeSet::new())
            .unwrap_err();
        assert_eq!(error.to_string(), "backupのhashが一致しません。");
        assert_eq!(
            fs::read(staging.path().join(name)).unwrap(),
            b"replacement fixture"
        );
    }
}
