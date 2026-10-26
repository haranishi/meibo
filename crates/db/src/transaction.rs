use std::ops::Deref;

use crate::{Connection, Result};

/// A transaction sharing its original connection and rolling back on drop.
///
/// The native SQLite guard also rolls back if committing fails, for example
/// because a deferred foreign-key constraint is violated.
#[derive(Debug)]
pub struct Transaction<'conn> {
    connection: &'conn Connection,
    local: rusqlite::Transaction<'conn>,
}

impl<'conn> Transaction<'conn> {
    pub(crate) const fn from_local(
        connection: &'conn Connection,
        local: rusqlite::Transaction<'conn>,
    ) -> Self {
        Self { connection, local }
    }

    /// Commit this transaction, rolling back on failure as SQLite normally does.
    ///
    /// # Errors
    /// Returns SQLite's commit error, including deferred constraint violations.
    pub fn commit(self) -> Result<()> {
        self.local.commit()
    }

    /// Roll back this transaction explicitly.
    ///
    /// # Errors
    /// Returns SQLite's rollback error.
    pub fn rollback(self) -> Result<()> {
        self.local.rollback()
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection
    }
}
