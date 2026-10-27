use meibo_core::{
    Engine, RemoteIdentity,
    remote_snapshot::{MAX_ROW_BYTES, encode_crm_sequences, encode_row},
    types::{DataOrigin, EngineError},
};
use meibo_db::{
    Connection, Row, params_from_iter,
    types::{Value, ValueRef},
};
use std::{io::Write, path::Path};

use super::{
    Database, HostedResult, INVALID, Manifest, Table, Tenant, ValidatedSnapshot,
    files::{self, RowWriter},
    manifest::{MAX_MANIFEST_BYTES, Sequences, Service},
    profile::{self, Profile},
    validate,
};
use crate::hosted::remote::{operation::RemoteOperation, store::RemoteStore};

pub(super) fn run(
    store: &RemoteStore,
    operation: &mut RemoteOperation,
    destination: &Path,
) -> HostedResult<ValidatedSnapshot> {
    store.check_ready()?;
    let registrations = store.registered_tenants()?;
    let root = files::new_directory(destination)?;
    files::new_directory(&root.join("control"))?;
    files::new_directory(&root.join("tenants"))?;
    let control = control(store, &root)?;
    let mut tenants = Vec::with_capacity(registrations.len());
    for (slug, identity) in registrations {
        files::new_directory(&root.join("tenants").join(&slug))?;
        let engine = operation.construction(store.open_tenant(&slug))?;
        let result = tenant(&engine, &root, &slug, &identity);
        operation.observe(&engine);
        tenants.push(Tenant {
            slug,
            database: result?,
        });
    }
    let manifest = Manifest {
        version: 1,
        service: Service {
            instance: store.instance().into(),
            database_id: store.database_id().into(),
        },
        control,
        tenants,
    };
    let mut file = files::new_file(&root.join("manifest.json"))?;
    // The manifest writer is bounded during encoding, before growing a Vec.
    let mut output = ManifestOutput {
        file: &mut file,
        written: 0,
    };
    serde_json::to_writer(&mut output, &manifest)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    let snapshot = validate(&root)?;
    if snapshot.manifest().service.database_id != store.database_id() {
        return Err(INVALID.into());
    }
    Ok(snapshot)
}

struct ManifestOutput<'a> {
    file: &'a mut std::fs::File,
    written: usize,
}
impl Write for ManifestOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.written = self
            .written
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_MANIFEST_BYTES)
            .ok_or_else(|| std::io::Error::other(INVALID))?;
        self.file.write_all(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn control(store: &RemoteStore, root: &Path) -> HostedResult<Database> {
    let tx = store.conn.unchecked_transaction()?;
    let mut tables = Vec::with_capacity(profile::CONTROL.len());
    for profile in profile::CONTROL {
        let expected: i64 = tx.query_row(
            &format!("SELECT count(*) FROM {}", profile.name),
            [],
            |row| row.get(0),
        )?;
        let mut writer = RowWriter::new(root, "control", profile.name)?;
        scan_control(&tx, *profile, &mut writer)?;
        let table = writer.finish()?;
        if u64::try_from(expected)? != table.rows {
            return Err(INVALID.into());
        }
        tables.push(table);
    }
    tx.rollback()?;
    Ok(Database {
        database_id: store.database_id().into(),
        database_kind: "control".into(),
        schema_version: 1,
        data_origin: None,
        sequences: None,
        tables,
    })
}

fn source_row(row: &Row<'_>, columns: usize) -> HostedResult<Vec<Value>> {
    let mut length = 0_usize;
    for index in 0..columns {
        let size = match row.get_ref(index)? {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
            _ => 8,
        };
        length = length
            .checked_add(size)
            .filter(|size| *size <= MAX_ROW_BYTES)
            .ok_or(INVALID)?;
    }
    Ok((0..columns)
        .map(|index| row.get(index))
        .collect::<meibo_db::Result<Vec<_>>>()?)
}

// Only compiled control profiles enter SQL. Eight one-MiB rows fit the adapter
// response/decode bounds; encoded rows are written and dropped individually.
fn scan_control(
    connection: &Connection,
    profile: Profile,
    writer: &mut RowWriter,
) -> HostedResult<()> {
    let keys = profile.keys.join(",");
    let placeholders = vec!["?"; profile.keys.len()].join(",");
    let mut after: Option<Vec<Value>> = None;
    loop {
        let continuation = if after.is_some() {
            format!(" WHERE ({keys}) > ({placeholders})")
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT {} FROM {}{continuation} ORDER BY {keys} LIMIT 8",
            profile.columns.join(","),
            profile.name
        );
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(after.as_deref().unwrap_or_default()))?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let values = source_row(row, profile.columns.len())?;
            let key = profile.key(&values)?;
            writer.write(&encode_row(&values)?)?;
            after = Some(key);
            count += 1;
        }
        if count < 8 {
            return Ok(());
        }
    }
}

fn tenant(
    engine: &Engine,
    root: &Path,
    slug: &str,
    identity: &RemoteIdentity,
) -> HostedResult<Database> {
    let prefix = format!("tenants/{slug}");
    let mut writers = profile::crm()
        .map(|table| RowWriter::new(root, &prefix, table.name))
        .collect::<HostedResult<Vec<_>>>()?;
    let sequences = engine.visit_crm_snapshot(|table, bytes| {
        let index = profile::crm()
            .position(|profile| profile.name == table.name())
            .expect("compiled CRM table");
        writers[index]
            .write(bytes)
            .map_err(|_| EngineError::Storage(Box::new(std::io::Error::other(INVALID))))
    })?;
    let sequences: Sequences = serde_json::from_slice(&encode_crm_sequences(sequences)?)?;
    let tables = writers
        .into_iter()
        .map(RowWriter::finish)
        .collect::<HostedResult<Vec<Table>>>()?;
    Ok(Database {
        database_id: identity.database_id().into(),
        database_kind: "crm".into(),
        schema_version: 1,
        data_origin: Some(
            match engine.origin() {
                DataOrigin::Real => "real",
                DataOrigin::Synthetic => "synthetic",
            }
            .into(),
        ),
        sequences: Some(sequences),
        tables,
    })
}
