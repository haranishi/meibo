use crate::{Error, MappedRows, Params, RemoteConnection, Result, Row, Rows};

enum Local<'conn> {
    Prepared(rusqlite::Statement<'conn>),
    Cached(rusqlite::CachedStatement<'conn>),
}

/// A prepared SQL statement, optionally retained in the connection's cache.
pub struct Statement<'conn> {
    backend: Backend<'conn>,
}

enum Backend<'conn> {
    Local(Local<'conn>),
    Remote {
        connection: &'conn RemoteConnection,
        sql: String,
        parameters: usize,
        columns: usize,
    },
}

impl<'conn> Statement<'conn> {
    pub(crate) fn from_local(statement: rusqlite::Statement<'conn>) -> Self {
        Self {
            backend: Backend::Local(Local::Prepared(statement)),
        }
    }

    pub(crate) fn from_cached(statement: rusqlite::CachedStatement<'conn>) -> Self {
        Self {
            backend: Backend::Local(Local::Cached(statement)),
        }
    }

    pub(crate) fn from_remote(
        connection: &'conn RemoteConnection,
        sql: String,
        parameters: usize,
        columns: usize,
    ) -> Self {
        Self {
            backend: Backend::Remote {
                connection,
                sql,
                parameters,
                columns,
            },
        }
    }

    /// Return the number of result columns in this statement.
    #[must_use]
    pub fn column_count(&self) -> usize {
        match &self.backend {
            Backend::Local(Local::Prepared(statement)) => statement.column_count(),
            Backend::Local(Local::Cached(statement)) => statement.column_count(),
            Backend::Remote { columns, .. } => *columns,
        }
    }

    /// Execute this statement and return its affected row count.
    ///
    /// # Errors
    /// Returns parameter conversion/binding or SQLite execution errors.
    pub fn execute<P: Params>(&mut self, params: P) -> Result<usize> {
        let params = params.into_values()?;
        match &mut self.backend {
            Backend::Local(local) => local
                .statement()
                .execute(rusqlite::params_from_iter(params)),
            Backend::Remote {
                connection,
                sql,
                parameters,
                ..
            } => {
                check_parameters(params.len(), *parameters)?;
                let result = connection.execute_values(sql, params, false)?;
                if !result.cols.is_empty() {
                    return Err(Error::ExecuteReturnedResults);
                }
                usize::try_from(result.affected_row_count)
                    .map_err(|_| crate::remote::failure("invalid remote database response"))
            }
        }
    }

    /// Bind parameters and lazily step through result rows.
    ///
    /// # Errors
    /// Returns parameter conversion or binding errors.
    pub fn query<P: Params>(&mut self, params: P) -> Result<Rows<'_>> {
        let params = params.into_values()?;
        match &mut self.backend {
            Backend::Local(local) => local
                .statement()
                .query(rusqlite::params_from_iter(params))
                .map(Rows::from_local),
            Backend::Remote {
                connection,
                sql,
                parameters,
                ..
            } => {
                check_parameters(params.len(), *parameters)?;
                Ok(Rows::from_remote(
                    connection.execute_values(sql, params, true)?,
                ))
            }
        }
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
        match &mut self.backend {
            Backend::Local(local) => {
                let rows = local
                    .statement()
                    .query(rusqlite::params_from_iter(params))?;
                Ok(MappedRows::from_local(rows, map))
            }
            Backend::Remote {
                connection,
                sql,
                parameters,
                ..
            } => {
                check_parameters(params.len(), *parameters)?;
                Ok(MappedRows::from_remote(
                    Rows::from_remote(connection.execute_values(sql, params, true)?),
                    map,
                ))
            }
        }
    }

    /// Return whether this statement yields at least one result row.
    ///
    /// # Errors
    /// Returns parameter conversion/binding or SQLite execution errors.
    pub fn exists<P: Params>(&mut self, params: P) -> Result<bool> {
        self.query(params)?.next().map(|row| row.is_some())
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
        match &mut self.backend {
            Backend::Local(local) => local
                .statement()
                .query_row(rusqlite::params_from_iter(params), |row| {
                    map(&Row::from_local(row))
                }),
            Backend::Remote {
                connection,
                sql,
                parameters,
                ..
            } => {
                check_parameters(params.len(), *parameters)?;
                let mut rows = Rows::from_remote(connection.execute_values(sql, params, true)?);
                map(rows.next()?.ok_or(Error::QueryReturnedNoRows)?)
            }
        }
    }
}

impl<'conn> Local<'conn> {
    fn statement(&mut self) -> &mut rusqlite::Statement<'conn> {
        match self {
            Self::Prepared(statement) => statement,
            Self::Cached(statement) => statement,
        }
    }
}

fn check_parameters(actual: usize, expected: usize) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(Error::InvalidParameterCount(actual, expected))
    }
}
