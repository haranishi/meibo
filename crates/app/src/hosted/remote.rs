//! Remote control storage; HTTP/CLI integration is a separate task.
pub(super) mod config;
pub(super) mod store;

#[cfg(all(test, feature = "integration-tests"))]
mod tests;
