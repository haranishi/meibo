use meibo_core::remote_snapshot::decode_crm_sequences;
use meibo_db::{Connection, TransactionBehavior, params, params_from_iter, types::Value};

use super::{HostedResult, INVALID, Identities, RemoteStore, ValidatedSnapshot};
use crate::hosted::remote_backup::{
    Tenant,
    profile::{self, Profile},
};

pub(super) const CONTROL_MARKERS: &[&str] = &[
    "remote_format",
    "schema_version",
    "database_id",
    "database_kind",
    "instance",
    "state",
    "dummy_hash",
];

pub(super) fn control(
    snapshot: &ValidatedSnapshot,
    target: &mut RemoteStore,
    identities: &Identities,
) -> HostedResult<()> {
    let tx = target
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    for profile in profile::CONTROL {
        let sql = insert_sql(*profile);
        let mut statement = tx.prepare(&sql)?;
        snapshot.visit_rows(None, profile.name, |values| {
            if profile.name == "meta" {
                metadata(&tx, values, CONTROL_MARKERS)?;
            } else if profile.name == "remote_tenants" {
                let [Value::Text(slug), _] = values else {
                    return Err(INVALID.into());
                };
                statement.execute(params![
                    slug,
                    identities.get(slug).ok_or(INVALID)?.database_id()
                ])?;
            } else {
                statement.execute(params_from_iter(values))?;
            }
            Ok(())
        })?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn tenant(
    snapshot: &ValidatedSnapshot,
    tenant: &Tenant,
    connection: &mut Connection,
) -> HostedResult<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM counts", [])?;
    for profile in profile::crm() {
        let sql = insert_sql(profile);
        let mut statement = tx.prepare(&sql)?;
        snapshot.visit_rows(Some(&tenant.slug), profile.name, |values| {
            if profile.name == "meta" {
                metadata(&tx, values, &["database_id"])?;
            } else {
                statement.execute(params_from_iter(values))?;
            }
            Ok(())
        })?;
    }
    let sequences = tenant.database.sequences.as_ref().ok_or(INVALID)?;
    let maximum: i64 = tx.query_row(
        "SELECT coalesce(max(id),0) FROM custom_field_definitions",
        [],
        |row| row.get(0),
    )?;
    let sequences = decode_crm_sequences(&serde_json::to_vec(sequences)?, maximum)?;
    tx.execute(
        "DELETE FROM sqlite_sequence WHERE name='custom_field_definitions'",
        [],
    )?;
    tx.execute(
        "INSERT INTO sqlite_sequence(name,seq) VALUES('custom_field_definitions',?1)",
        [sequences.high_water()],
    )?;
    tx.execute("INSERT INTO name_fts(name_fts) VALUES('rebuild')", [])?;
    tx.execute(
        "INSERT INTO name_fts(name_fts,rank) VALUES('integrity-check',1)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

fn metadata(connection: &Connection, values: &[Value], retained: &[&str]) -> HostedResult<()> {
    let [Value::Text(key), Value::Text(value)] = values else {
        return Err(INVALID.into());
    };
    if !retained.contains(&key.as_str()) {
        connection.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
    }
    Ok(())
}

fn insert_sql(profile: Profile) -> String {
    format!(
        "INSERT INTO {}({}) VALUES({})",
        profile.name,
        profile.columns.join(","),
        vec!["?"; profile.columns.len()].join(",")
    )
}
