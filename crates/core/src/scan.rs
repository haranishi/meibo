//! Trusted, bounded keyset iteration within the caller's read transaction.

use crate::{
    query::storage,
    sql::{
        Connection, Row, params_from_iter,
        types::{Value, ValueRef},
    },
    types::EngineError,
};

pub(crate) const PAGE_ROWS: u32 = 500;

// `base` and key names come only from compiled schema/SQL definitions. A false
// visitor result stops the scan; no additional page is requested after failure.
pub(crate) fn visit_rows(
    connection: &Connection,
    base: &str,
    keys: &[(&str, usize)],
    page_rows: u32,
    mut visitor: impl FnMut(&Row<'_>) -> Result<bool, EngineError>,
) -> Result<bool, EngineError> {
    assert!(
        (1..=PAGE_ROWS).contains(&page_rows),
        "compiled scan page bound"
    );
    let names = keys
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = vec!["?"; keys.len()].join(",");
    let mut after: Option<Vec<Value>> = None;
    loop {
        let continuation = if after.is_some() {
            format!(" AND ({names}) > ({placeholders})")
        } else {
            String::new()
        };
        let sql = format!("{base}{continuation} ORDER BY {names} LIMIT {page_rows}");
        let mut statement = connection.prepare(&sql).map_err(storage)?;
        let mut rows = statement
            .query(params_from_iter(after.as_deref().unwrap_or_default()))
            .map_err(storage)?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(storage)? {
            for (_, index) in keys {
                if matches!(row.get_ref(*index).map_err(storage)?, ValueRef::Null) {
                    return Err(EngineError::Corrupt);
                }
            }
            if !visitor(row)? {
                return Ok(false);
            }
            let key = keys
                .iter()
                .map(|(_, index)| row.get::<_, Value>(*index))
                .collect::<crate::sql::Result<Vec<_>>>()
                .map_err(storage)?;
            after = Some(key);
            count += 1;
        }
        if count < page_rows {
            return Ok(true);
        }
    }
}
