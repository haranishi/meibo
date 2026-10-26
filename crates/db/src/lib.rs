//! Synchronous SQL adapter with typed values and a shared runtime.

mod error;
mod local;
mod params;
mod remote;
mod row;
mod rows;
mod statement;
mod transaction;

pub use error::{Error, ErrorCode, OptionalExtension, Result};
pub use local::Connection;
pub use params::{Params, ParamsFromIter, params_from_iter};
pub use remote::{RemoteConnection, RemoteOptions};
pub use row::{Row, RowIndex};
pub use rows::{MappedRows, Rows};
pub use rusqlite::{MAIN_DB, OpenFlags, TransactionBehavior, params, types};
pub use statement::Statement;
pub use transaction::Transaction;
pub use types::{FromSql, ToSql, Value, ValueRef};

/// Progress information for local file backup and restore callbacks.
pub mod backup {
    pub use rusqlite::backup::Progress;
}

use std::{io, sync::OnceLock};
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<io::Result<Runtime>> = OnceLock::new();

/// Return the process-wide runtime used by synchronous database operations.
///
/// The runtime keeps its IO and timer drivers alive between operations and can
/// drive independent requests from multiple synchronous threads.
///
/// # Errors
/// Returns an IO error if the runtime's worker threads or drivers cannot start.
pub fn runtime() -> io::Result<&'static Runtime> {
    RUNTIME
        .get_or_init(|| {
            Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("meibo-db")
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|error| io::Error::new(error.kind(), "database runtime unavailable"))
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    #[test]
    fn runtime_is_shared_and_drives_timers_on_multiple_sync_threads() {
        let runtime = super::runtime().unwrap();
        thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|value| {
                    scope.spawn(move || {
                        let other = super::runtime().unwrap();
                        assert!(std::ptr::eq(runtime, other));
                        other.block_on(async {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            value
                        })
                    })
                })
                .collect();
            let values: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect();
            assert_eq!(values, vec![0, 1, 2, 3]);
        });
    }
}
