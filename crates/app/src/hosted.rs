//! 招待制サービスの認証と組織別データ境界。
mod auth;
mod backup;
mod cli;
mod control;
mod http;
mod public_entry;
#[cfg(feature = "cloud")]
mod remote;
#[cfg(feature = "cloud")]
mod remote_backup;
#[cfg(feature = "cloud")]
mod remote_gate;
mod store;

#[cfg(test)]
use auth::Login;
use auth::{Identity, LoginOutcome, Role};
#[cfg(test)]
use http::HostedReply;
use http::{Hosted, HostedConfig};
use std::{
    error::Error,
    ffi::OsString,
    time::{SystemTime, UNIX_EPOCH},
};
use store::Store;

#[cfg(feature = "cloud")]
use meibo_db as sql;
#[cfg(not(feature = "cloud"))]
use rusqlite as sql;

type HostedResult<T> = std::result::Result<T, Box<dyn Error>>;

pub(crate) fn run(args: Vec<OsString>) -> HostedResult<()> {
    cli::run(args)
}

#[cfg(feature = "cloud")]
pub(crate) fn run_remote(args: Vec<OsString>) -> HostedResult<()> {
    remote::run(args)
}

fn now() -> HostedResult<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

#[cfg(test)]
mod tests;
