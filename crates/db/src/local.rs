use std::{path::Path, time::Duration};

use crate::{
    OpenFlags, Params, RemoteConnection, RemoteErrorKind, RemoteOptions, Result, Row, Statement,
    ToSql, Transaction, TransactionBehavior,
};

/// A synchronous SQL connection to an explicit local or remote backend.
#[derive(Debug)]
pub struct Connection {
    backend: Backend,
}

#[derive(Debug)]
enum Backend {
    Local(rusqlite::Connection),
    Remote(RemoteConnection),
}

impl Connection {
    /// Construct the explicitly selected external database backend.
    ///
    /// # Errors
    /// Returns a sanitized connection error when unavailable or invalid.
    pub fn open_remote(options: RemoteOptions) -> Result<Self> {
        RemoteConnection::connect(options).map(|remote| Self {
            backend: Backend::Remote(remote),
        })
    }

    /// Whether this connection uses an explicitly selected external database.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        matches!(self.backend, Backend::Remote(_))
    }

    /// Whether a communication failure has made subsequent operations unsafe.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        match &self.backend {
            Backend::Local(_) => false,
            Backend::Remote(remote) => remote.is_poisoned(),
        }
    }

    /// Open or create a local SQLite database.
    ///
    /// # Errors
    /// Returns SQLite's file-opening or initialization error.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        rusqlite::Connection::open(path).map(|local| Self {
            backend: Backend::Local(local),
        })
    }

    /// Open a local database with SQLite's explicit access flags.
    ///
    /// # Errors
    /// Returns SQLite's file-opening or initialization error.
    pub fn open_with_flags<P: AsRef<Path>>(path: P, flags: OpenFlags) -> Result<Self> {
        rusqlite::Connection::open_with_flags(path, flags).map(|local| Self {
            backend: Backend::Local(local),
        })
    }

    /// Return the local main database path; remote databases have no local path.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match &self.backend {
            Backend::Local(local) => local.path(),
            Backend::Remote(_) => None,
        }
    }

    /// Set the local connection's lock wait timeout.
    ///
    /// # Errors
    /// Returns SQLite's error, or Unsupported for a remote backend.
    pub fn busy_timeout(&self, timeout: Duration) -> Result<()> {
        self.local()?.busy_timeout(timeout)
    }

    /// Set a local SQLite pragma, optionally in an attached schema.
    ///
    /// # Errors
    /// Returns SQLite's error, or Unsupported for a remote backend.
    pub fn pragma_update<V: ToSql>(
        &self,
        schema: Option<&str>,
        name: &str,
        value: V,
    ) -> Result<()> {
        self.local()?.pragma_update(schema, name, value)
    }

    /// Read a local SQLite pragma with the same row conversion as a query.
    ///
    /// # Errors
    /// Returns SQLite/conversion errors, or Unsupported for a remote backend.
    pub fn pragma_query_value<T, F>(&self, schema: Option<&str>, name: &str, map: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        self.local()?
            .pragma_query_value(schema, name, |row| map(&Row::from_local(row)))
    }

    /// Prepare SQL for repeated execution on this connection.
    ///
    /// # Errors
    /// Returns SQL preparation or remote connection errors.
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        match &self.backend {
            Backend::Local(local) => local.prepare(sql).map(Statement::from_local),
            Backend::Remote(remote) => remote.prepare(sql),
        }
    }

    /// Prepare SQL using the backend's reusable statement facility.
    ///
    /// # Errors
    /// Returns SQL preparation or remote connection errors.
    pub fn prepare_cached(&self, sql: &str) -> Result<Statement<'_>> {
        match &self.backend {
            Backend::Local(local) => local.prepare_cached(sql).map(Statement::from_cached),
            Backend::Remote(remote) => remote.prepare_cached(sql),
        }
    }

    /// Execute a statement and return its affected row count.
    ///
    /// # Errors
    /// Returns SQL, parameter conversion/binding or execution errors.
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize> {
        match &self.backend {
            Backend::Local(local) => {
                let params = params.into_values()?;
                local.execute(sql, rusqlite::params_from_iter(params))
            }
            Backend::Remote(remote) => remote.execute(sql, params),
        }
    }

    /// Execute a trusted SQL batch, including complete trigger definitions.
    ///
    /// # Errors
    /// Returns SQL execution errors or unsupported remote transaction control.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        match &self.backend {
            Backend::Local(local) => local.execute_batch(sql),
            Backend::Remote(remote) => remote.execute_batch(sql),
        }
    }

    /// Convert the first result row, ignoring any additional rows.
    ///
    /// # Errors
    /// Returns SQL, parameter or conversion errors, or `QueryReturnedNoRows`.
    pub fn query_row<T, P, F>(&self, sql: &str, params: P, map: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        match &self.backend {
            Backend::Local(local) => {
                let params = params.into_values()?;
                local.query_row(sql, rusqlite::params_from_iter(params), |row| {
                    map(&Row::from_local(row))
                })
            }
            Backend::Remote(remote) => remote.query_row(sql, params, map),
        }
    }

    /// Begin a deferred transaction, rolling back unless committed.
    ///
    /// # Errors
    /// Returns the backend's transaction-start error.
    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.unchecked_transaction()
    }

    /// Begin a deferred transaction, checking nesting at runtime.
    ///
    /// Original connection operations share this transaction and snapshot.
    ///
    /// # Errors
    /// Returns transaction-start, nested transaction or connection errors.
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        self.begin(TransactionBehavior::Deferred)
    }

    /// Begin a transaction with an explicit SQLite locking behavior.
    ///
    /// # Errors
    /// Returns the backend's transaction-start or lock error.
    pub fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        self.begin(behavior)
    }

    fn begin(&self, behavior: TransactionBehavior) -> Result<Transaction<'_>> {
        match &self.backend {
            Backend::Local(local) => {
                let transaction = rusqlite::Transaction::new_unchecked(local, behavior)?;
                Ok(Transaction::from_local(self, transaction))
            }
            Backend::Remote(remote) => {
                remote.begin(behavior)?;
                Ok(Transaction::from_remote(self))
            }
        }
    }

    pub(crate) fn finish_remote(&self, commit: bool) -> Result<()> {
        match &self.backend {
            Backend::Remote(remote) => remote.finish(commit),
            Backend::Local(_) => Err(RemoteErrorKind::Unsupported.error()),
        }
    }

    /// Return the last inserted row id on this connection's write stream.
    #[must_use]
    pub fn last_insert_rowid(&self) -> i64 {
        match &self.backend {
            Backend::Local(local) => local.last_insert_rowid(),
            Backend::Remote(remote) => remote.last_insert_rowid(),
        }
    }

    /// Whether the connection is known to be outside a transaction.
    /// A poisoned remote connection has unknown state and returns false.
    #[must_use]
    pub fn is_autocommit(&self) -> bool {
        match &self.backend {
            Backend::Local(local) => local.is_autocommit(),
            Backend::Remote(remote) => remote.is_autocommit(),
        }
    }

    /// Back up a local database, including committed WAL contents.
    ///
    /// # Errors
    /// Returns SQLite's error, or Unsupported for a remote backend.
    pub fn backup<N: rusqlite::Name, P: AsRef<Path>>(
        &self,
        name: N,
        destination: P,
        progress: Option<fn(crate::backup::Progress)>,
    ) -> Result<()> {
        self.local()?.backup(name, destination, progress)
    }

    /// Restore a local database from a SQLite backup file.
    ///
    /// # Errors
    /// Returns SQLite's error, or Unsupported for a remote backend.
    pub fn restore<N: rusqlite::Name, P: AsRef<Path>, F: Fn(crate::backup::Progress)>(
        &mut self,
        name: N,
        source: P,
        progress: Option<F>,
    ) -> Result<()> {
        match &mut self.backend {
            Backend::Local(local) => local.restore(name, source, progress),
            Backend::Remote(_) => Err(RemoteErrorKind::Unsupported.error()),
        }
    }

    fn local(&self) -> Result<&rusqlite::Connection> {
        match &self.backend {
            Backend::Local(local) => Ok(local),
            Backend::Remote(_) => Err(RemoteErrorKind::Unsupported.error()),
        }
    }
}
