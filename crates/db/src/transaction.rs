use std::ops::Deref;

use crate::{Connection, Result};

/// A transaction sharing its original connection and rolling back on drop.
/// Native SQLite rollback behavior is retained for local files.
#[derive(Debug)]
pub struct Transaction<'conn> {
    connection: &'conn Connection,
    backend: Option<Backend<'conn>>,
}

#[derive(Debug)]
enum Backend<'conn> {
    Local(rusqlite::Transaction<'conn>),
    Remote,
}

impl<'conn> Transaction<'conn> {
    pub(crate) const fn from_local(
        connection: &'conn Connection,
        local: rusqlite::Transaction<'conn>,
    ) -> Self {
        Self {
            connection,
            backend: Some(Backend::Local(local)),
        }
    }

    pub(crate) const fn from_remote(connection: &'conn Connection) -> Self {
        Self {
            connection,
            backend: Some(Backend::Remote),
        }
    }

    /// Commit once; a known commit failure attempts one rollback.
    /// An unknown remote outcome poisons the connection without resending.
    ///
    /// # Errors
    /// Returns the backend commit error or explicit remote `OutcomeUnknown`.
    pub fn commit(mut self) -> Result<()> {
        match self.backend.take() {
            Some(Backend::Local(local)) => local.commit(),
            Some(Backend::Remote) => self.connection.finish_remote(true),
            None => unreachable!(),
        }
    }

    /// Roll back explicitly once, without another attempt from Drop.
    ///
    /// # Errors
    /// Returns the backend rollback error or explicit remote `OutcomeUnknown`.
    pub fn rollback(mut self) -> Result<()> {
        match self.backend.take() {
            Some(Backend::Local(local)) => local.rollback(),
            Some(Backend::Remote) => self.connection.finish_remote(false),
            None => unreachable!(),
        }
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        self.connection
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if matches!(self.backend, Some(Backend::Remote)) {
            self.backend.take();
            // A lost cleanup response poisons the shared state. finish refuses
            // any request if another operation already left an unknown outcome.
            let _ = self.connection.finish_remote(false);
        }
        // The remaining native guard owns local rollback exactly as before.
    }
}
