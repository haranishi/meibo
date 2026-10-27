use super::{HostedResult, INVALID};
use meibo_core::remote_snapshot::crm_tables;
use meibo_db::types::Value;

#[derive(Clone, Copy)]
pub(super) struct Profile {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub keys: &'static [&'static str],
}

pub(super) const CONTROL: &[Profile] = &[
    Profile {
        name: "meta",
        columns: &["key", "value"],
        keys: &["key"],
    },
    Profile {
        name: "users",
        columns: &["id", "username", "password_hash", "disabled"],
        keys: &["id"],
    },
    Profile {
        name: "tenants",
        columns: &["slug"],
        keys: &["slug"],
    },
    Profile {
        name: "memberships",
        columns: &["user_id", "tenant", "role"],
        keys: &["user_id", "tenant"],
    },
    Profile {
        name: "audit",
        columns: &["id", "at", "actor", "tenant", "operation", "outcome"],
        keys: &["id"],
    },
    Profile {
        name: "login_attempts",
        columns: &["bucket", "window_start", "failures"],
        keys: &["bucket"],
    },
    Profile {
        name: "remote_tenants",
        columns: &["slug", "database_id"],
        keys: &["slug"],
    },
];

pub(super) fn crm() -> impl Iterator<Item = Profile> {
    crm_tables().iter().map(|table| Profile {
        name: table.name(),
        columns: table.columns(),
        keys: table.key_columns(),
    })
}

impl Profile {
    pub fn indices(self) -> Vec<usize> {
        self.keys
            .iter()
            .map(|key| {
                self.columns
                    .iter()
                    .position(|column| column == key)
                    .expect("compiled snapshot key")
            })
            .collect()
    }
    pub fn key(self, values: &[Value]) -> HostedResult<Vec<Value>> {
        self.keys
            .iter()
            .zip(self.indices())
            .map(|(name, index)| {
                let value = &values[index];
                let valid = match *name {
                    "key" | "slug" | "tenant" | "bucket" => matches!(value, Value::Text(_)),
                    "number_hash" => matches!(value, Value::Blob(_)),
                    _ => matches!(value, Value::Integer(_)),
                };
                if valid {
                    Ok(value.clone())
                } else {
                    Err(INVALID.into())
                }
            })
            .collect()
    }
}
