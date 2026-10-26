use std::{path::Path, time::Duration};

use crate::{OpenFlags, Params, Result, Row, Statement, ToSql, Transaction, TransactionBehavior};

/// A synchronous SQL connection. Local files use SQLite directly.
#[derive(Debug)]
pub struct Connection {
    local: rusqlite::Connection,
}

impl Connection {
    /// Open or create a local SQLite database.
    ///
    /// # Errors
    /// Returns SQLite's file-opening or initialization error.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        rusqlite::Connection::open(path).map(|local| Self { local })
    }

    /// Open a local database with SQLite's explicit access flags.
    ///
    /// # Errors
    /// Returns SQLite's file-opening or initialization error.
    pub fn open_with_flags<P: AsRef<Path>>(path: P, flags: OpenFlags) -> Result<Self> {
        rusqlite::Connection::open_with_flags(path, flags).map(|local| Self { local })
    }

    /// Return the local main database path, as reported by SQLite.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.local.path()
    }

    /// Set the local connection's lock wait timeout.
    ///
    /// # Errors
    /// Returns SQLite's busy-handler configuration error.
    pub fn busy_timeout(&self, timeout: Duration) -> Result<()> {
        self.local.busy_timeout(timeout)
    }

    /// Set a local SQLite pragma, optionally in an attached schema.
    ///
    /// # Errors
    /// Returns pragma construction, value conversion or SQLite execution errors.
    pub fn pragma_update<V: ToSql>(
        &self,
        schema: Option<&str>,
        name: &str,
        value: V,
    ) -> Result<()> {
        self.local.pragma_update(schema, name, value)
    }

    /// Read a local SQLite pragma with the same row conversion as a query.
    ///
    /// # Errors
    /// Returns pragma, SQLite execution or row conversion errors.
    pub fn pragma_query_value<T, F>(&self, schema: Option<&str>, name: &str, map: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        self.local
            .pragma_query_value(schema, name, |row| map(&Row::from_local(row)))
    }

    /// Prepare SQL for repeated execution on this connection.
    ///
    /// # Errors
    /// Returns SQLite's SQL preparation error.
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        self.local.prepare(sql).map(Statement::from_local)
    }

    /// Prepare SQL using SQLite's connection-owned statement cache.
    ///
    /// # Errors
    /// Returns SQLite's SQL preparation error.
    pub fn prepare_cached(&self, sql: &str) -> Result<Statement<'_>> {
        self.local.prepare_cached(sql).map(Statement::from_cached)
    }

    /// Execute a statement and return its affected row count.
    ///
    /// # Errors
    /// Returns SQL, parameter conversion/binding or execution errors.
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize> {
        let params = params.into_values()?;
        self.local.execute(sql, rusqlite::params_from_iter(params))
    }

    /// Execute a trusted SQL batch, including complete trigger definitions.
    ///
    /// # Errors
    /// Returns SQLite's batch execution error.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.local.execute_batch(sql)
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
        let params = params.into_values()?;
        self.local
            .query_row(sql, rusqlite::params_from_iter(params), |row| {
                map(&Row::from_local(row))
            })
    }

    /// Begin a deferred transaction, rolling back unless committed.
    ///
    /// # Errors
    /// Returns SQLite's transaction-start error.
    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.unchecked_transaction()
    }

    /// Begin a deferred transaction with nesting checked by SQLite at runtime.
    ///
    /// The original connection shares the transaction and its read snapshot.
    ///
    /// # Errors
    /// Returns SQLite's transaction-start error, including nested transactions.
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        let local = self.local.unchecked_transaction()?;
        Ok(Transaction::from_local(self, local))
    }

    /// Begin a transaction with an explicit SQLite locking behavior.
    ///
    /// # Errors
    /// Returns SQLite's transaction-start or lock error.
    pub fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        let local = rusqlite::Transaction::new_unchecked(&self.local, behavior)?;
        Ok(Transaction::from_local(self, local))
    }

    /// Return the last inserted row id on this connection.
    #[must_use]
    pub fn last_insert_rowid(&self) -> i64 {
        self.local.last_insert_rowid()
    }

    /// Return whether SQLite is currently outside a transaction.
    #[must_use]
    pub fn is_autocommit(&self) -> bool {
        self.local.is_autocommit()
    }

    /// Back up a local database, including committed WAL contents.
    ///
    /// # Errors
    /// Returns SQLite's destination-opening or backup error.
    pub fn backup<N: rusqlite::Name, P: AsRef<Path>>(
        &self,
        name: N,
        destination: P,
        progress: Option<fn(crate::backup::Progress)>,
    ) -> Result<()> {
        self.local.backup(name, destination, progress)
    }

    /// Restore a local database from a SQLite backup file.
    ///
    /// # Errors
    /// Returns SQLite's source-opening or restore error.
    pub fn restore<N: rusqlite::Name, P: AsRef<Path>, F: Fn(crate::backup::Progress)>(
        &mut self,
        name: N,
        source: P,
        progress: Option<F>,
    ) -> Result<()> {
        self.local.restore(name, source, progress)
    }
}
