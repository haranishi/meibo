//! 招待制サービスの認証と組織別データ境界。
mod auth;
mod backup;
mod cli;
mod http;
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

type HostedResult<T> = std::result::Result<T, Box<dyn Error>>;

pub(crate) fn run(args: Vec<OsString>) -> HostedResult<()> {
    cli::run(args)
}

fn now() -> HostedResult<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

#[cfg(test)]
mod tests;
