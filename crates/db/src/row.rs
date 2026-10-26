use std::{fmt, sync::Arc};

use crate::{Error, FromSql, Result, Value, ValueRef, error, types::FromSqlError};

enum Data<'stmt> {
    Local(&'stmt rusqlite::Row<'stmt>),
    BufferedLocal {
        columns: Arc<[String]>,
        values: Vec<BufferedValue>,
    },
    Owned {
        columns: Arc<[String]>,
        values: Vec<Value>,
    },
}

// SQLite text need not be UTF-8, and local REAL expressions may be infinite.
// Preserve their raw representation until the requested FromSql conversion.
enum BufferedValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl From<ValueRef<'_>> for BufferedValue {
    fn from(value: ValueRef<'_>) -> Self {
        match value {
            ValueRef::Null => Self::Null,
            ValueRef::Integer(value) => Self::Integer(value),
            ValueRef::Real(value) => Self::Real(value),
            ValueRef::Text(value) => Self::Text(value.to_vec()),
            ValueRef::Blob(value) => Self::Blob(value.to_vec()),
        }
    }
}

impl BufferedValue {
    fn as_ref(&self) -> ValueRef<'_> {
        match self {
            Self::Null => ValueRef::Null,
            Self::Integer(value) => ValueRef::Integer(*value),
            Self::Real(value) => ValueRef::Real(*value),
            Self::Text(value) => ValueRef::Text(value),
            Self::Blob(value) => ValueRef::Blob(value),
        }
    }
}

/// A borrowed SQLite row or a row of owned values from a bounded response.
pub struct Row<'stmt> {
    data: Data<'stmt>,
}

impl<'stmt> Row<'stmt> {
    /// Borrow a local row while SQLite keeps its current result valid.
    #[must_use]
    pub const fn from_local(row: &'stmt rusqlite::Row<'stmt>) -> Self {
        Self {
            data: Data::Local(row),
        }
    }

    pub(crate) fn from_buffered_local(
        columns: Arc<[String]>,
        row: &rusqlite::Row<'_>,
    ) -> Result<Self> {
        let values = (0..columns.len())
            .map(|index| row.get_ref(index).map(BufferedValue::from))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            data: Data::BufferedLocal { columns, values },
        })
    }

    /// Construct an owned row, sharing its column names with other result rows.
    ///
    /// # Errors
    /// Rejects mismatched column/value counts and non-finite real values.
    pub fn from_values(columns: Arc<[String]>, values: Vec<Value>) -> Result<Self> {
        if columns.len() != values.len() {
            return Err(Error::InvalidQuery);
        }
        for (index, value) in values.iter().enumerate() {
            ensure_finite(ValueRef::from(value), index, &columns[index])?;
        }
        Ok(Self {
            data: Data::Owned { columns, values },
        })
    }

    /// Read a typed value by its zero-based index or column name.
    ///
    /// # Errors
    /// Preserves SQLite's invalid-index/name, type, range and conversion errors.
    pub fn get<I: RowIndex, T: FromSql>(&self, index: I) -> Result<T> {
        let index = index.index(self)?;
        let value = self.get_ref(index)?;
        T::column_result(value).map_err(|error| {
            error::from_sql(error, index, self.column_name(index), value.data_type())
        })
    }

    /// Borrow the underlying typed value without copying its text or blob.
    ///
    /// # Errors
    /// Returns invalid-index/name errors. Local cells retain SQLite's raw value.
    pub fn get_ref<I: RowIndex>(&self, index: I) -> Result<ValueRef<'_>> {
        let index = index.index(self)?;
        Ok(match &self.data {
            Data::Local(row) => row.get_ref(index)?,
            Data::BufferedLocal { values, .. } => values[index].as_ref(),
            Data::Owned { values, .. } => ValueRef::from(&values[index]),
        })
    }

    fn column_count(&self) -> usize {
        match &self.data {
            Data::Local(row) => row.as_ref().column_count(),
            Data::Owned { columns, .. } | Data::BufferedLocal { columns, .. } => columns.len(),
        }
    }

    fn column_name(&self, index: usize) -> &str {
        match &self.data {
            // Both sources are indexed only after RowIndex has checked bounds.
            Data::Local(row) => row.as_ref().column_name(index).unwrap(),
            Data::Owned { columns, .. } | Data::BufferedLocal { columns, .. } => &columns[index],
        }
    }
}

fn ensure_finite(value: ValueRef<'_>, index: usize, name: &str) -> Result<()> {
    if matches!(value, ValueRef::Real(real) if !real.is_finite()) {
        return Err(error::from_sql(
            FromSqlError::other(error::non_finite_real()),
            index,
            name,
            value.data_type(),
        ));
    }
    Ok(())
}

impl fmt::Debug for Row<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Row")
            .field("column_count", &self.column_count())
            .finish_non_exhaustive()
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for usize {}
    impl Sealed for &str {}
}

/// A zero-based column index (`usize`) or a column name (`&str`).
pub trait RowIndex: sealed::Sealed + Copy {
    /// Resolve a column while checking it exists in this row.
    ///
    /// # Errors
    /// Returns an invalid-index or invalid-name error for missing columns.
    #[doc(hidden)]
    fn index(&self, row: &Row<'_>) -> Result<usize>;
}

impl RowIndex for usize {
    fn index(&self, row: &Row<'_>) -> Result<usize> {
        if *self < row.column_count() {
            Ok(*self)
        } else {
            Err(Error::InvalidColumnIndex(*self))
        }
    }
}

impl RowIndex for &str {
    fn index(&self, row: &Row<'_>) -> Result<usize> {
        match &row.data {
            Data::Local(row) => row.as_ref().column_index(self),
            Data::Owned { columns, .. } | Data::BufferedLocal { columns, .. } => columns
                .iter()
                .position(|column| column.eq_ignore_ascii_case(self))
                .ok_or_else(|| Error::InvalidColumnName((*self).into())),
        }
    }
}
