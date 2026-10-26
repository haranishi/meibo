//! Keep the domain's SQL error variants and optional-row behavior compatible.

pub use rusqlite::{Error, ErrorCode, OptionalExtension, Result};

use crate::types::{FromSqlError, Type};

pub(crate) fn from_sql(error: FromSqlError, index: usize, name: &str, kind: Type) -> Error {
    match error {
        FromSqlError::InvalidType => Error::InvalidColumnType(index, name.into(), kind),
        FromSqlError::OutOfRange(value) => Error::IntegralValueOutOfRange(index, value),
        FromSqlError::Utf8Error(error) => Error::Utf8Error(index, error),
        FromSqlError::Other(error) => Error::FromSqlConversionFailure(index, kind, error),
        error => Error::FromSqlConversionFailure(index, kind, Box::new(error)),
    }
}

pub(crate) fn non_finite_real() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "SQL real must be finite")
}
