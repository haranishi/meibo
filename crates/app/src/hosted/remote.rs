//! Remote control storage and the shared hosted command/HTTP integration.
mod cli;
pub(super) mod config;
pub(super) mod operation;
pub(super) mod store;

pub(super) fn run(args: Vec<std::ffi::OsString>) -> super::HostedResult<()> {
    operation::report(cli::run(args))
}

#[cfg(all(test, feature = "integration-tests"))]
mod runtime_tests;
#[cfg(all(test, feature = "integration-tests"))]
mod tests;
