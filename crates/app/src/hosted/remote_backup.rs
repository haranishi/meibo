//! Bounded logical snapshots. All SQL and physical column order are compiled in.
use std::{error::Error, fmt, path::Path};

use super::{
    HostedResult,
    remote::{operation::RemoteOperation, store::RemoteStore},
};

mod create;
mod files;
mod manifest;
mod profile;
mod restore;
mod validate;

pub(in crate::hosted) use manifest::{Manifest, SnapshotDatabase as Database, Table, Tenant};
pub(in crate::hosted) use validate::{ValidatedSnapshot, validate};

const INVALID: &str = "リモートバックアップの保存形式・保存先を確認してください。";

struct BackupError(Box<dyn Error>);
impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(INVALID)
    }
}
impl fmt::Debug for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteBackupError")
    }
}
impl Error for BackupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn redacted<T>(result: HostedResult<T>) -> HostedResult<T> {
    result.map_err(|error| Box::new(BackupError(error)) as Box<dyn Error>)
}

/// Caller owns the one service gate and its completion protocol.
pub(in crate::hosted) fn create(
    store: &RemoteStore,
    operation: &mut RemoteOperation,
    destination: &Path,
) -> HostedResult<ValidatedSnapshot> {
    redacted(create::run(store, operation, destination))
}

pub(in crate::hosted) fn restore(
    store: &RemoteStore,
    operation: &mut RemoteOperation,
    snapshot: &Path,
    target: super::remote::config::Endpoints,
) -> HostedResult<()> {
    redacted(restore::run(store, operation, snapshot, target))
}

#[cfg(test)]
mod tests;
