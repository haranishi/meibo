use super::{
    Database, HostedResult, INVALID, Manifest, Table, files,
    manifest::{MAX_MANIFEST_BYTES, MAX_TENANTS},
    profile::{self, Profile},
    redacted,
};
use crate::hosted::{
    remote::store::{opaque_id, valid_dummy_hash},
    store::valid_slug,
};
use meibo_core::{
    RemoteIdentity,
    remote_snapshot::{MAX_ROW_BYTES, decode_crm_sequences, decode_row},
};
use meibo_db::types::Value;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, Read},
    path::{Path, PathBuf},
};

/// Fully checked manifest plus a reader that rechecks every subsequently read
/// file. It retains no row collection and executes no SQL from snapshot data.
pub(in crate::hosted) struct ValidatedSnapshot {
    root: PathBuf,
    manifest: Manifest,
}
impl ValidatedSnapshot {
    pub(in crate::hosted) fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// `None` selects control; a slug selects one registered CRM. The visitor
    /// receives canonical decoded values in the compiled physical column order.
    pub(in crate::hosted) fn visit_rows(
        &self,
        slug: Option<&str>,
        table: &str,
        visitor: impl FnMut(&[Value]) -> HostedResult<()>,
    ) -> HostedResult<()> {
        redacted((|| {
            let (database, profiles) = if let Some(slug) = slug {
                let tenant = self
                    .manifest
                    .tenants
                    .iter()
                    .find(|tenant| tenant.slug == slug)
                    .ok_or(INVALID)?;
                (&tenant.database, profile::crm().collect::<Vec<_>>())
            } else {
                (&self.manifest.control, profile::CONTROL.to_vec())
            };
            let profile = profiles
                .into_iter()
                .find(|profile| profile.name == table)
                .ok_or(INVALID)?;
            let table = database
                .tables
                .iter()
                .find(|table| table.name == profile.name)
                .ok_or(INVALID)?;
            read_rows(&self.root, table, profile, visitor)
        })())
    }
}

/// Check every manifest field, table, row, metadata marker, sequence and digest
/// before a caller starts writing any restore target.
pub(in crate::hosted) fn validate(root: &Path) -> HostedResult<ValidatedSnapshot> {
    redacted((|| {
        let root = files::checked_directory(root)?;
        let bytes = files::bounded_read(&root, "manifest.json", MAX_MANIFEST_BYTES)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        header(&manifest)?;
        let snapshot = ValidatedSnapshot { root, manifest };
        contents(&snapshot)?;
        Ok(snapshot)
    })())
}

fn header(manifest: &Manifest) -> HostedResult<()> {
    if manifest.version != 1
        || manifest.tenants.len() > MAX_TENANTS
        || !opaque_id(&manifest.service.instance)
        || !opaque_id(&manifest.service.database_id)
        || manifest.control.database_id != manifest.service.database_id
        || manifest.control.database_kind != "control"
        || manifest.control.schema_version != 1
        || manifest.control.data_origin.is_some()
        || manifest.control.sequences.is_some()
    {
        return Err(INVALID.into());
    }
    tables(
        &manifest.control,
        "control",
        profile::CONTROL.iter().copied(),
    )?;
    let mut identities = BTreeSet::from([manifest.control.database_id.as_str()]);
    let mut previous: Option<&str> = None;
    for tenant in &manifest.tenants {
        let database = &tenant.database;
        if !valid_slug(&tenant.slug)
            || previous.is_some_and(|previous| previous >= tenant.slug.as_str())
            || database.database_kind != "crm"
            || database.schema_version != 1
            || !matches!(database.data_origin.as_deref(), Some("real" | "synthetic"))
            || database.sequences.is_none()
            || !identities.insert(&database.database_id)
        {
            return Err(INVALID.into());
        }
        RemoteIdentity::new(database.database_id.clone(), "crm")?;
        tables(
            database,
            &format!("tenants/{}", tenant.slug),
            profile::crm(),
        )?;
        previous = Some(&tenant.slug);
    }
    Ok(())
}

fn tables(
    database: &Database,
    prefix: &str,
    profiles: impl Iterator<Item = Profile>,
) -> HostedResult<()> {
    let mut count = 0;
    for (index, profile) in profiles.enumerate() {
        let table = database.tables.get(index).ok_or(INVALID)?;
        if table.name != profile.name
            || table.file != format!("{prefix}/{}.jsonl", profile.name)
            || table.sha256.len() != 64
            || !table
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(INVALID.into());
        }
        count += 1;
    }
    if count != database.tables.len() {
        return Err(INVALID.into());
    }
    Ok(())
}

fn contents(snapshot: &ValidatedSnapshot) -> HostedResult<()> {
    let root = &snapshot.root;
    entries(root, ["manifest.json", "control", "tenants"])?;
    entries(
        &root.join("control"),
        profile::CONTROL
            .iter()
            .map(|profile| format!("{}.jsonl", profile.name)),
    )?;
    entries(
        &root.join("tenants"),
        snapshot
            .manifest
            .tenants
            .iter()
            .map(|tenant| tenant.slug.as_str()),
    )?;
    check_database(snapshot, None, &snapshot.manifest.control)?;
    registrations(snapshot, "tenants")?;
    registrations(snapshot, "remote_tenants")?;
    for tenant in &snapshot.manifest.tenants {
        entries(
            &root.join("tenants").join(&tenant.slug),
            profile::crm().map(|profile| format!("{}.jsonl", profile.name)),
        )?;
        check_database(snapshot, Some(&tenant.slug), &tenant.database)?;
    }
    Ok(())
}

fn entries(path: &Path, expected: impl IntoIterator<Item = impl AsRef<str>>) -> HostedResult<()> {
    files::checked_directory(path)?;
    let mut expected: BTreeSet<String> = expected
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(INVALID)?;
        if !expected.remove(name) || entry.file_type()?.is_symlink() {
            return Err(INVALID.into());
        }
    }
    if !expected.is_empty() {
        return Err(INVALID.into());
    }
    Ok(())
}

fn check_database(
    snapshot: &ValidatedSnapshot,
    slug: Option<&str>,
    database: &Database,
) -> HostedResult<()> {
    let mut metadata = BTreeMap::new();
    let mut maximum = None;
    for table in &database.tables {
        snapshot.visit_rows(slug, &table.name, |values| {
            if table.name == "meta" {
                metadata_row(&mut metadata, values)?;
            }
            if table.name == "custom_field_definitions" {
                let Value::Integer(id) = values[0] else {
                    return Err(INVALID.into());
                };
                maximum = Some(id);
            }
            Ok(())
        })?;
    }
    metadata_check(snapshot, slug, database, &metadata)?;
    if let Some(sequences) = &database.sequences {
        decode_crm_sequences(&serde_json::to_vec(sequences)?, maximum.unwrap_or(0))?;
    }
    Ok(())
}

const MARKERS: &[&str] = &[
    "remote_format",
    "schema_version",
    "database_id",
    "database_kind",
    "instance",
    "state",
    "dummy_hash",
    "data_origin",
    "created_at",
    "crm_schema_version",
    "custom_fields_schema_version",
    "public_data_schema_version",
];
fn metadata_row(metadata: &mut BTreeMap<String, String>, values: &[Value]) -> HostedResult<()> {
    let [Value::Text(key), Value::Text(value)] = values else {
        return Err(INVALID.into());
    };
    if MARKERS.contains(&key.as_str())
        && (value.len() > 1024 || metadata.insert(key.clone(), value.clone()).is_some())
    {
        return Err(INVALID.into());
    }
    Ok(())
}

fn metadata_check(
    snapshot: &ValidatedSnapshot,
    slug: Option<&str>,
    database: &Database,
    metadata: &BTreeMap<String, String>,
) -> HostedResult<()> {
    let value = |key: &str| metadata.get(key).map(String::as_str).ok_or(INVALID);
    if value("database_id")? != database.database_id
        || value("database_kind")? != database.database_kind
        || value("schema_version")? != "1"
    {
        return Err(INVALID.into());
    }
    if slug.is_none() {
        if value("remote_format")? != "meibo-control-remote-v1"
            || value("state")? != "ready"
            || value("instance")? != snapshot.manifest.service.instance
            || !valid_dummy_hash(value("dummy_hash")?)
        {
            return Err(INVALID.into());
        }
    } else {
        if value("remote_format")? != "meibo-remote-v1"
            || Some(value("data_origin")?) != database.data_origin.as_deref()
        {
            return Err(INVALID.into());
        }
        for key in [
            "crm_schema_version",
            "custom_fields_schema_version",
            "public_data_schema_version",
        ] {
            if value(key)? != "1" {
                return Err(INVALID.into());
            }
        }
        let created = value("created_at")?;
        if created
            .parse::<u64>()
            .ok()
            .is_none_or(|number| number.to_string() != created)
        {
            return Err(INVALID.into());
        }
    }
    Ok(())
}

fn registrations(snapshot: &ValidatedSnapshot, table: &str) -> HostedResult<()> {
    let mut next = 0;
    snapshot.visit_rows(None, table, |values| {
        let tenant = snapshot.manifest.tenants.get(next).ok_or(INVALID)?;
        if values.first() != Some(&Value::Text(tenant.slug.clone()))
            || (table == "remote_tenants"
                && values.get(1) != Some(&Value::Text(tenant.database.database_id.clone())))
        {
            return Err(INVALID.into());
        }
        next += 1;
        Ok(())
    })?;
    if next != snapshot.manifest.tenants.len() {
        return Err(INVALID.into());
    }
    Ok(())
}

fn read_rows(
    root: &Path,
    table: &Table,
    profile: Profile,
    mut visitor: impl FnMut(&[Value]) -> HostedResult<()>,
) -> HostedResult<()> {
    let mut reader = files::open_file(root, &table.file)?;
    let mut bytes = Vec::new();
    let mut digest = Sha256::new();
    let mut count = 0_u64;
    let mut previous: Option<Vec<Value>> = None;
    loop {
        bytes.clear();
        (&mut reader)
            .take(u64::try_from(MAX_ROW_BYTES + 2)?)
            .read_until(b'\n', &mut bytes)?;
        if bytes.is_empty() {
            break;
        }
        if bytes.last() != Some(&b'\n') || bytes.len() > MAX_ROW_BYTES + 1 {
            return Err(INVALID.into());
        }
        digest.update(&bytes);
        let values = decode_row(&bytes[..bytes.len() - 1], profile.columns.len())?;
        let key = profile.key(&values)?;
        if previous
            .as_deref()
            .is_some_and(|previous| compare(previous, &key) != Ordering::Less)
        {
            return Err(INVALID.into());
        }
        previous = Some(key);
        count = count
            .checked_add(1)
            .filter(|count| *count <= table.rows)
            .ok_or(INVALID)?;
        visitor(&values)?;
    }
    if count != table.rows || format!("{:x}", digest.finalize()) != table.sha256 {
        return Err(INVALID.into());
    }
    Ok(())
}

fn compare(left: &[Value], right: &[Value]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let order = match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
            (Value::Text(left), Value::Text(right)) => left.as_bytes().cmp(right.as_bytes()),
            (Value::Blob(left), Value::Blob(right)) => left.cmp(right),
            _ => return Ordering::Equal,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}
