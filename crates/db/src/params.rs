use crate::{Error, Result, ToSql, Value, error, types::ToSqlOutput};

mod sealed {
    pub trait Sealed {}
}

/// Positional SQL parameters accepted by the adapter.
///
/// Supports `params!`, `[]`, arrays of up to 32 values, tuples of up to five
/// values, and the iterator returned by [`params_from_iter`].
pub trait Params: sealed::Sealed {
    /// Convert parameters into owned, typed values without numeric coercion.
    ///
    /// # Errors
    /// Returns conversion errors, including invalid UTF-8 or non-finite reals.
    fn into_values(self) -> Result<Vec<Value>>;
}

pub(crate) fn to_value<T: ToSql + ?Sized>(parameter: &T) -> Result<Value> {
    let value = match parameter.to_sql()? {
        ToSqlOutput::Borrowed(value) => Value::try_from(value)
            .map_err(|error| Error::ToSqlConversionFailure(Box::new(error)))?,
        ToSqlOutput::Owned(value) => value,
        _ => {
            return Err(Error::ToSqlConversionFailure(Box::new(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsupported SQL parameter representation",
                ),
            )));
        }
    };
    if matches!(value, Value::Real(real) if !real.is_finite()) {
        return Err(Error::ToSqlConversionFailure(Box::new(
            error::non_finite_real(),
        )));
    }
    Ok(value)
}

// As in rusqlite, this must be the only by-value empty-array implementation so
// the application's untyped `[]` arguments remain inferable.
impl sealed::Sealed for [&(dyn ToSql + Send + Sync); 0] {}
impl Params for [&(dyn ToSql + Send + Sync); 0] {
    fn into_values(self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

impl sealed::Sealed for () {}
impl Params for () {
    fn into_values(self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

impl sealed::Sealed for &[&dyn ToSql] {}
impl Params for &[&dyn ToSql] {
    fn into_values(self) -> Result<Vec<Value>> {
        self.iter().map(to_value).collect()
    }
}

impl<T: ToSql + ?Sized, const N: usize> sealed::Sealed for &[&T; N] {}
impl<T: ToSql + ?Sized, const N: usize> Params for &[&T; N] {
    fn into_values(self) -> Result<Vec<Value>> {
        self.iter().map(to_value).collect()
    }
}

macro_rules! array_params {
    ($($size:literal),+ $(,)?) => {$(
        impl<T: ToSql> sealed::Sealed for [T; $size] {}
        impl<T: ToSql> Params for [T; $size] {
            fn into_values(self) -> Result<Vec<Value>> {
                self.iter().map(to_value).collect()
            }
        }
    )+};
}

array_params!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
);

macro_rules! tuple_params {
    ($($type:ident: $field:tt),+ $(,)?) => {
        impl<$($type: ToSql),+> sealed::Sealed for ($($type,)+) {}
        impl<$($type: ToSql),+> Params for ($($type,)+) {
            fn into_values(self) -> Result<Vec<Value>> {
                Ok(vec![$(to_value(&self.$field)?),+])
            }
        }
    };
}

tuple_params!(A: 0);
tuple_params!(A: 0, B: 1);
tuple_params!(A: 0, B: 1, C: 2);
tuple_params!(A: 0, B: 1, C: 2, D: 3);
tuple_params!(A: 0, B: 1, C: 2, D: 3, E: 4);

/// An iterator of positional parameters, constructed by [`params_from_iter`].
#[derive(Clone, Debug)]
pub struct ParamsFromIter<I>(I);

/// Use a dynamically sized iterator as positional SQL parameters.
#[must_use]
pub const fn params_from_iter<I>(iter: I) -> ParamsFromIter<I> {
    ParamsFromIter(iter)
}

impl<I> sealed::Sealed for ParamsFromIter<I> {}
impl<I> Params for ParamsFromIter<I>
where
    I: IntoIterator,
    I::Item: ToSql,
{
    fn into_values(self) -> Result<Vec<Value>> {
        self.0.into_iter().map(|value| to_value(&value)).collect()
    }
}
