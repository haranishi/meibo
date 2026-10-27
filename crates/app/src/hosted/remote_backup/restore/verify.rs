use meibo_core::{
    normalize,
    remote_snapshot::{encode_row, read_crm_sequences},
};
use meibo_db::{Connection, params_from_iter, types::Value};
use sha2::{Digest, Sha256};

use super::{HostedResult, INVALID, Identities, ValidatedSnapshot, rows::CONTROL_MARKERS};
use crate::hosted::remote_backup::profile::{self, Profile};

#[derive(Default)]
struct DigestRows {
    rows: u64,
    digest: Sha256,
}
impl DigestRows {
    fn visit(&mut self, slug: Option<&str>, table: &str, values: &[Value]) -> HostedResult<()> {
        self.rows = self.rows.checked_add(1).ok_or(INVALID)?;
        if table == "meta" {
            let Some(Value::Text(key)) = values.first() else {
                return Err(INVALID.into());
            };
            if (slug.is_none() && CONTROL_MARKERS.contains(&key.as_str()))
                || (slug.is_some() && key == "database_id")
            {
                return Ok(());
            }
        }
        self.digest.update(encode_row(values)?);
        self.digest.update(b"\n");
        Ok(())
    }
    fn finish(self) -> (u64, [u8; 32]) {
        (self.rows, self.digest.finalize().into())
    }
}

pub(super) fn database(
    snapshot: &ValidatedSnapshot,
    slug: Option<&str>,
    connection: &Connection,
    identities: &Identities,
) -> HostedResult<()> {
    let tx = connection.unchecked_transaction()?;
    let integrity: String = tx.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    let foreign: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get(0),
    )?;
    if integrity != "ok" || foreign {
        return Err(INVALID.into());
    }
    let profiles = if slug.is_some() {
        profile::crm().collect::<Vec<_>>()
    } else {
        profile::CONTROL.to_vec()
    };
    for profile in profiles {
        let mut expected = DigestRows::default();
        snapshot.visit_rows(slug, profile.name, |values| {
            if slug.is_none() && profile.name == "remote_tenants" {
                let [Value::Text(slug), _] = values else {
                    return Err(INVALID.into());
                };
                expected.visit(
                    None,
                    profile.name,
                    &[
                        Value::Text(slug.clone()),
                        Value::Text(identities.get(slug).ok_or(INVALID)?.database_id().into()),
                    ],
                )
            } else {
                expected.visit(slug, profile.name, values)
            }
        })?;
        let actual = table(&tx, slug, profile)?;
        if actual.finish() != expected.finish() {
            return Err(INVALID.into());
        }
    }
    if let Some(slug) = slug {
        let tenant = snapshot
            .manifest()
            .tenants
            .iter()
            .find(|tenant| tenant.slug == slug)
            .ok_or(INVALID)?;
        let expected = tenant
            .database
            .sequences
            .as_ref()
            .ok_or(INVALID)?
            .custom_field_definitions
            .as_str();
        if read_crm_sequences(&tx)?.high_water().to_string() != expected {
            return Err(INVALID.into());
        }
    } else {
        let empty: bool = tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM sessions) AND NOT EXISTS(SELECT 1 FROM remote_gate)",
            [],
            |row| row.get(0),
        )?;
        if !empty {
            return Err(INVALID.into());
        }
    }
    tx.rollback()?;
    Ok(())
}

fn table(
    connection: &Connection,
    slug: Option<&str>,
    profile: Profile,
) -> HostedResult<DigestRows> {
    let mut result = DigestRows::default();
    let mut after: Option<Vec<Value>> = None;
    let keys = profile.keys.join(",");
    let placeholders = vec!["?"; profile.keys.len()].join(",");
    loop {
        let continuation = if after.is_some() {
            format!(" WHERE ({keys}) > ({placeholders})")
        } else {
            String::new()
        };
        let mut statement = connection.prepare(&format!(
            "SELECT {} FROM {}{continuation} ORDER BY {keys} LIMIT 8",
            profile.columns.join(","),
            profile.name
        ))?;
        let mut rows = statement.query(params_from_iter(after.as_deref().unwrap_or_default()))?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let values = (0..profile.columns.len())
                .map(|index| row.get::<_, Value>(index))
                .collect::<meibo_db::Result<Vec<_>>>()?;
            if profile.name == "companies" {
                normalized(&values)?;
            }
            result.visit(slug, profile.name, &values)?;
            after = Some(profile.key(&values)?);
            count += 1;
        }
        if count < 8 {
            return Ok(result);
        }
    }
}

fn normalized(values: &[Value]) -> HostedResult<()> {
    let (Value::Text(name), Value::Text(key)) = (&values[2], &values[3]) else {
        return Err(INVALID.into());
    };
    if normalize::name_key(name) != *key {
        return Err(INVALID.into());
    }
    Ok(())
}
