use std::sync::Arc;

use crate::{Result, Row};

/// A lazy cursor holding only its current owned row between calls to `next`.
#[must_use = "rows are lazy and do nothing unless consumed"]
pub struct Rows<'stmt> {
    local: rusqlite::Rows<'stmt>,
    columns: Arc<[String]>,
    current: Option<Row<'stmt>>,
}

impl<'stmt> Rows<'stmt> {
    pub(crate) fn from_local(local: rusqlite::Rows<'stmt>) -> Self {
        Self {
            local,
            columns: Arc::from([]),
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
        if let Some(row) = self.local.next()? {
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
        Ok(self.current.as_ref())
    }
}

/// A lazy iterator applying a conversion to each current SQLite row.
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct MappedRows<'stmt, F> {
    local: rusqlite::Rows<'stmt>,
    map: F,
}

impl<'stmt, F> MappedRows<'stmt, F> {
    pub(crate) fn from_local(local: rusqlite::Rows<'stmt>, map: F) -> Self {
        Self { local, map }
    }
}

impl<T, F> Iterator for MappedRows<'_, F>
where
    F: FnMut(&Row<'_>) -> Result<T>,
{
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.local
            .next()
            .transpose()
            .map(|row| row.and_then(|row| (self.map)(&Row::from_local(row))))
    }
}
