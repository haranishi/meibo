//! Explicit remote CRM lifecycle and identity checks.

use std::collections::BTreeMap;

use meibo_db::RemoteOptions;

use super::{
    Connection, Db, Integrity, OpenOptions, TransactionBehavior, VERSION, check_integrity,
    check_version, counts, origin_text, storage, unix_seconds, validate_options,
};
use crate::types::{DataOrigin, EngineError};

const FORMAT: &str = "meibo-remote-v1";
const KIND: &str = "crm";
const METADATA: &str = "SELECT key,value FROM meta WHERE key IN (
 'remote_format','schema_version','database_id','database_kind','data_origin',
 'created_at','crm_schema_version','custom_fields_schema_version','public_data_schema_version')";
// Preparing this read-only statement checks that required base and extension
// tables exist without running initializers or reading company contents.
const SCHEMA_PROBE: &str = "SELECT companies.id,companies.has_phone,contacts.id,activities.id,
 lists.id,list_members.list_id,counts.n,erase_gate.x,name_fts.name_key,
 crm_work_items.id,crm_opportunities.id,custom_field_definitions.id,
 custom_field_values.field_id,erased_corporate_numbers.number_hash
 FROM companies,contacts,activities,lists,list_members,counts,erase_gate,name_fts,
 crm_work_items,crm_opportunities,custom_field_definitions,custom_field_values,
 erased_corporate_numbers WHERE 0";

/// Server-owned identity of one remote database, independent of its URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIdentity {
    database_id: String,
    database_kind: String,
}

impl RemoteIdentity {
    /// Validate an opaque identifier and a database-kind label.
    ///
    /// # Errors
    /// Returns `InvalidInput` for empty, oversized or non-identifier values.
    pub fn new(
        database_id: impl Into<String>,
        database_kind: impl Into<String>,
    ) -> Result<Self, EngineError> {
        let database_id = database_id.into();
        let database_kind = database_kind.into();
        if !identifier(&database_id, 128) || !identifier(&database_kind, 32) {
            return Err(EngineError::InvalidInput {
                field: "remote_identity".into(),
                reason: "bounded opaque identifiers required".into(),
            });
        }
        Ok(Self {
            database_id,
            database_kind,
        })
    }

    /// Recorded opaque database identifier.
    #[must_use]
    pub fn database_id(&self) -> &str {
        &self.database_id
    }

    /// Recorded database-kind label; a CRM Engine requires `crm`.
    #[must_use]
    pub fn database_kind(&self) -> &str {
        &self.database_kind
    }
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl Db {
    pub(crate) fn create_remote(
        endpoint: RemoteOptions,
        identity: &RemoteIdentity,
        origin: DataOrigin,
        opt: &OpenOptions,
    ) -> Result<Self, EngineError> {
        validate_config(identity, opt)?;
        let mut conn = Connection::open_remote(endpoint).map_err(storage)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let occupied: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if occupied {
            return Err(EngineError::InvalidInput {
                field: "remote_storage".into(),
                reason: "empty database required".into(),
            });
        }
        tx.execute_batch(include_str!("ddl.sql")).map_err(storage)?;
        for (key, value) in [
            ("remote_format", FORMAT.to_owned()),
            ("schema_version", VERSION.to_string()),
            ("database_id", identity.database_id.clone()),
            ("database_kind", identity.database_kind.clone()),
            ("data_origin", origin_text(origin).to_owned()),
            ("created_at", unix_seconds()?.to_string()),
        ] {
            tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2)", (key, value))
                .map_err(storage)?;
        }
        crate::crm::initialize(&tx)?;
        crate::custom_fields::initialize(&tx)?;
        crate::public_data::initialize(&tx)?;
        counts::recount(&tx)?;
        tx.commit().map_err(storage)?;
        Ok(Self {
            conn,
            checkpoint: None,
            origin,
            opt: opt.clone(),
        })
    }

    pub(crate) fn open_remote(
        endpoint: RemoteOptions,
        identity: &RemoteIdentity,
        opt: &OpenOptions,
    ) -> Result<Self, EngineError> {
        validate_config(identity, opt)?;
        let conn = Connection::open_remote(endpoint).map_err(storage)?;
        let tx = conn.unchecked_transaction().map_err(storage)?;
        let mut metadata = BTreeMap::new();
        let mut statement = tx.prepare(METADATA).map_err(storage)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage)?;
        for row in rows {
            let (key, value) = row.map_err(storage)?;
            if metadata.insert(key, value).is_some() {
                return Err(EngineError::Corrupt);
            }
        }
        drop(statement);
        let origin = validate_metadata(&metadata, identity)?;
        tx.prepare(SCHEMA_PROBE).map_err(storage)?;
        if opt.integrity == Integrity::Always {
            check_integrity(&tx, true)?;
        }
        tx.commit().map_err(storage)?;
        Ok(Self {
            conn,
            checkpoint: None,
            origin,
            opt: opt.clone(),
        })
    }
}

fn validate_config(identity: &RemoteIdentity, opt: &OpenOptions) -> Result<(), EngineError> {
    validate_options(opt)?;
    if identity.database_kind != KIND {
        return Err(EngineError::InvalidInput {
            field: "remote_identity".into(),
            reason: "CRM database kind required".into(),
        });
    }
    Ok(())
}

fn validate_metadata(
    metadata: &BTreeMap<String, String>,
    identity: &RemoteIdentity,
) -> Result<DataOrigin, EngineError> {
    let required = |key: &str| {
        metadata
            .get(key)
            .map(String::as_str)
            .ok_or(EngineError::Corrupt)
    };
    if required("remote_format")? != FORMAT
        || required("database_id")? != identity.database_id
        || required("database_kind")? != identity.database_kind
    {
        return Err(EngineError::Corrupt);
    }
    for (key, expected) in [
        ("schema_version", VERSION),
        ("crm_schema_version", crate::crm::EXTENSION_VERSION),
        (
            "custom_fields_schema_version",
            crate::custom_fields::VERSION,
        ),
        ("public_data_schema_version", crate::public_data::VERSION),
    ] {
        let raw = required(key)?;
        let found: u32 = raw.parse().map_err(|_| EngineError::Corrupt)?;
        if found.to_string() != raw {
            return Err(EngineError::Corrupt);
        }
        check_version(found, expected)?;
    }
    let created = required("created_at")?;
    let value: u64 = created.parse().map_err(|_| EngineError::Corrupt)?;
    if value.to_string() != created {
        return Err(EngineError::Corrupt);
    }
    match required("data_origin")? {
        "synthetic" => Ok(DataOrigin::Synthetic),
        "real" => Ok(DataOrigin::Real),
        _ => Err(EngineError::Corrupt),
    }
}
