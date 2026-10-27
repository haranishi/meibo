use std::{collections::BTreeMap, path::Path};

use meibo_core::{Engine, RemoteIdentity, types::DataOrigin};
use meibo_db::{Connection, RemoteOptions};

use super::{HostedResult, INVALID, ValidatedSnapshot, validate};
use crate::hosted::{
    auth,
    remote::{config::Endpoints, operation::RemoteOperation, store::RemoteStore},
    store::engine_options,
};

mod rows;
mod verify;

type Identities = BTreeMap<String, RemoteIdentity>;

pub(super) fn run(
    source: &RemoteStore,
    operation: &mut RemoteOperation,
    path: &Path,
    endpoints: Endpoints,
) -> HostedResult<()> {
    let snapshot = validate(path)?;
    preflight(source, operation, &snapshot, &endpoints)?;
    let identities = snapshot
        .manifest()
        .tenants
        .iter()
        .map(|tenant| {
            Ok((
                tenant.slug.clone(),
                RemoteIdentity::new(auth::random_hex(16)?, "crm")?,
            ))
        })
        .collect::<HostedResult<Identities>>()?;
    // This constructor commits restoring as part of the first control DDL.
    let tenant_endpoints = endpoints.clone();
    let mut target = operation.construction(RemoteStore::initialize_restoring(endpoints))?;
    let result = candidate(
        &snapshot,
        operation,
        &tenant_endpoints,
        &identities,
        &mut target,
    );
    operation.observe_uncertainty(target.conn.is_poisoned());
    result
}

fn preflight(
    source: &RemoteStore,
    operation: &mut RemoteOperation,
    snapshot: &ValidatedSnapshot,
    target: &Endpoints,
) -> HostedResult<()> {
    let manifest = snapshot.manifest();
    if !source.distinct_targets(target)
        || manifest.service.database_id != source.database_id()
        || manifest.service.instance != source.instance()
        || !target
            .tenants
            .keys()
            .eq(manifest.tenants.iter().map(|tenant| &tenant.slug))
    {
        return Err(INVALID.into());
    }
    let registrations = source.registered_tenants()?;
    if !registrations
        .iter()
        .map(|(slug, id)| (slug, id.database_id()))
        .eq(manifest
            .tenants
            .iter()
            .map(|tenant| (&tenant.slug, tenant.database.database_id.as_str())))
    {
        return Err(INVALID.into());
    }
    // Finish every check before creating even the control schema.
    for endpoint in std::iter::once(&target.control).chain(target.tenants.values()) {
        connected(operation, endpoint, |connection| {
            let tx = connection.unchecked_transaction()?;
            let occupied: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
                [],
                |row| row.get(0),
            )?;
            tx.rollback()?;
            if occupied {
                Err(INVALID.into())
            } else {
                Ok(())
            }
        })?;
    }
    Ok(())
}

fn candidate(
    snapshot: &ValidatedSnapshot,
    operation: &mut RemoteOperation,
    endpoints: &Endpoints,
    identities: &Identities,
    target: &mut RemoteStore,
) -> HostedResult<()> {
    rows::control(snapshot, target, identities)?;
    for tenant in &snapshot.manifest().tenants {
        let endpoint = &endpoints.tenants[&tenant.slug];
        let identity = &identities[&tenant.slug];
        let origin = match tenant.database.data_origin.as_deref() {
            Some("real") => DataOrigin::Real,
            Some("synthetic") => DataOrigin::Synthetic,
            _ => return Err(INVALID.into()),
        };
        let engine = operation.construction(
            Engine::create_remote(endpoint.clone(), identity, origin, &engine_options())
                .map_err(Into::into),
        )?;
        drop(engine);
        connected(operation, endpoint, |connection| {
            rows::tenant(snapshot, tenant, connection)?;
            verify::database(snapshot, Some(&tenant.slug), connection, identities)
        })?;
        let engine = operation.construction(
            Engine::open_remote(endpoint.clone(), identity, &engine_options()).map_err(Into::into),
        )?;
        let result = engine.verify();
        operation.observe(&engine);
        let report = result?;
        if !report.integrity_ok || !report.counts_match || !report.folds_match {
            return Err(INVALID.into());
        }
    }
    verify::database(snapshot, None, &target.conn, identities)?;
    let tx = target.conn.transaction()?;
    tx.execute("INSERT INTO audit(at,actor,tenant,operation,outcome) VALUES(?1,'maintenance',NULL,'restore','success')", [crate::hosted::now()?])?;
    if tx.execute(
        "UPDATE meta SET value='ready' WHERE key='state' AND value='restoring'",
        [],
    )? != 1
    {
        return Err(INVALID.into());
    }
    tx.commit()?;
    Ok(())
}

fn connected<T>(
    operation: &mut RemoteOperation,
    endpoint: &RemoteOptions,
    work: impl FnOnce(&mut Connection) -> HostedResult<T>,
) -> HostedResult<T> {
    let mut connection =
        operation.construction(Connection::open_remote(endpoint.clone()).map_err(Into::into))?;
    let result = work(&mut connection);
    operation.observe_uncertainty(connection.is_poisoned());
    result
}
