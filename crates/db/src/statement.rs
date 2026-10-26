use crate::{MappedRows, Params, Result, Row, Rows};

enum Local<'conn> {
    Prepared(rusqlite::Statement<'conn>),
    Cached(rusqlite::CachedStatement<'conn>),
}

/// A prepared SQL statement, optionally retained in the connection's cache.
pub struct Statement<'conn> {
    local: Local<'conn>,
}

impl<'conn> Statement<'conn> {
    pub(crate) fn from_local(statement: rusqlite::Statement<'conn>) -> Self {
        Self {
            local: Local::Prepared(statement),
        }
    }

    pub(crate) fn from_cached(statement: rusqlite::CachedStatement<'conn>) -> Self {
        Self {
            local: Local::Cached(statement),
        }
    }

    fn local_mut(&mut self) -> &mut rusqlite::Statement<'conn> {
        match &mut self.local {
            Local::Prepared(statement) => statement,
            Local::Cached(statement) => statement,
        }
    }

    /// Return the number of result columns in this statement.
    #[must_use]
    pub fn column_count(&self) -> usize {
        match &self.local {
            Local::Prepared(statement) => statement.column_count(),
            Local::Cached(statement) => statement.column_count(),
        }
    }

    /// Execute this statement and return its affected row count.
    ///
    /// # Errors
    /// Returns parameter conversion/binding or SQLite execution errors.
    pub fn execute<P: Params>(&mut self, params: P) -> Result<usize> {
        let params = params.into_values()?;
        self.local_mut().execute(rusqlite::params_from_iter(params))
    }

    /// Bind parameters and lazily step through result rows.
    ///
    /// # Errors
    /// Returns parameter conversion or binding errors.
    pub fn query<P: Params>(&mut self, params: P) -> Result<Rows<'_>> {
        let params = params.into_values()?;
        self.local_mut()
            .query(rusqlite::params_from_iter(params))
            .map(Rows::from_local)
    }

    /// Lazily map each result row, borrowing the current SQLite row.
    ///
    /// # Errors
    /// Returns parameter conversion or binding errors. The iterator reports
    /// execution and per-row conversion errors as its individual items.
    pub fn query_map<T, P, F>(&mut self, params: P, map: F) -> Result<MappedRows<'_, F>>
    where
        P: Params,
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        let params = params.into_values()?;
        let rows = self.local_mut().query(rusqlite::params_from_iter(params))?;
        Ok(MappedRows::from_local(rows, map))
    }

    /// Return whether this statement yields at least one result row.
    ///
    /// # Errors
    /// Returns parameter conversion/binding or SQLite execution errors.
    pub fn exists<P: Params>(&mut self, params: P) -> Result<bool> {
        let params = params.into_values()?;
        self.local_mut().exists(rusqlite::params_from_iter(params))
    }

    /// Convert the first result row, ignoring any additional rows.
    ///
    /// # Errors
    /// Returns parameter, execution or conversion errors, or `QueryReturnedNoRows`.
    pub fn query_row<T, P, F>(&mut self, params: P, map: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let params = params.into_values()?;
        self.local_mut()
            .query_row(rusqlite::params_from_iter(params), |row| {
                map(&Row::from_local(row))
            })
    }
}
