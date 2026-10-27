use std::sync::Arc;

use crate::{Result, Row};

/// A lazy cursor holding only its current owned row between calls to `next`.
#[must_use = "rows are lazy and do nothing unless consumed"]
pub struct Rows<'stmt> {
    backend: Backend<'stmt>,
    columns: Arc<[String]>,
    current: Option<Row<'stmt>>,
}

enum Backend<'stmt> {
    Local(rusqlite::Rows<'stmt>),
    Remote(std::vec::IntoIter<libsql_hrana::proto::Row>),
}

impl<'stmt> Rows<'stmt> {
    pub(crate) fn from_local(local: rusqlite::Rows<'stmt>) -> Self {
        Self {
            backend: Backend::Local(local),
            columns: Arc::from([]),
            current: None,
        }
    }

    pub(crate) fn from_remote(result: libsql_hrana::proto::StmtResult) -> Self {
        Self {
            backend: Backend::Remote(result.rows.into_iter()),
            columns: result
                .cols
                .into_iter()
                .map(|column| column.name.unwrap_or_default())
                .collect(),
            current: None,
        }
    }

    /// Step to the next row. Its borrow remains valid until the next step.
    ///
    /// # Errors
    /// Returns SQLite's step error. Cell conversions are deferred to `Row::get`.
    #[expect(clippy::should_implement_trait)] // A row borrows the cursor itself.
    pub fn next(&mut self) -> Result<Option<&Row<'stmt>>> {
        self.current = None;
        match &mut self.backend {
            Backend::Local(local) => {
                if let Some(row) = local.next()? {
                    let statement = row.as_ref();
                    // Read names after stepping: SQLite may have recompiled the query
                    // after a schema change since it was first prepared.
                    let names = statement.column_names();
                    if !self
                        .columns
                        .iter()
                        .map(String::as_str)
                        .eq(names.iter().copied())
                    {
                        self.columns = names.into_iter().map(str::to_owned).collect();
                    }
                    self.current = Some(Row::from_buffered_local(Arc::clone(&self.columns), row)?);
                }
            }
            Backend::Remote(rows) => {
                if let Some(row) = rows.next() {
                    let values = row
                        .values
                        .into_iter()
                        .map(crate::remote::decode_value)
                        .collect::<Result<Vec<_>>>()?;
                    self.current =
                        Some(Row::from_remote_values(Arc::clone(&self.columns), values)?);
                }
            }
        }
        Ok(self.current.as_ref())
    }
}

/// A lazy iterator applying a conversion to each current SQLite row.
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct MappedRows<'stmt, F> {
    backend: MappedBackend<'stmt>,
    map: F,
}

enum MappedBackend<'stmt> {
    Local(rusqlite::Rows<'stmt>),
    Remote(Rows<'stmt>),
}

impl<'stmt, F> MappedRows<'stmt, F> {
    pub(crate) fn from_local(local: rusqlite::Rows<'stmt>, map: F) -> Self {
        Self {
            backend: MappedBackend::Local(local),
            map,
        }
    }

    pub(crate) fn from_remote(rows: Rows<'stmt>, map: F) -> Self {
        Self {
            backend: MappedBackend::Remote(rows),
            map,
        }
    }
}

impl<T, F> Iterator for MappedRows<'_, F>
where
    F: FnMut(&Row<'_>) -> Result<T>,
{
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.backend {
            MappedBackend::Local(local) => local
                .next()
                .transpose()
                .map(|row| row.and_then(|row| (self.map)(&Row::from_local(row)))),
            MappedBackend::Remote(rows) => rows
                .next()
                .transpose()
                .map(|row| row.and_then(&mut self.map)),
        }
    }
}
