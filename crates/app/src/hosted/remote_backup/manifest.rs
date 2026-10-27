use serde::{Deserialize, Serialize};

pub(super) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub(super) const MAX_TENANTS: usize = 1000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct Manifest {
    pub version: u32,
    pub service: Service,
    pub control: SnapshotDatabase,
    pub tenants: Vec<Tenant>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct Service {
    pub instance: String,
    pub database_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct Tenant {
    pub slug: String,
    pub database: SnapshotDatabase,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct SnapshotDatabase {
    pub database_id: String,
    pub database_kind: String,
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequences: Option<Sequences>,
    pub tables: Vec<Table>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct Sequences {
    pub custom_field_definitions: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct Table {
    pub name: String,
    pub file: String,
    pub rows: u64,
    pub sha256: String,
}
