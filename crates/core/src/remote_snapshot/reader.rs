use super::{
    CrmSequences, CrmTable, MAX_ROW_BYTES, SnapshotError, crm_tables, encode_row,
    read_crm_sequences,
};
use crate::{
    query::storage,
    schema::Db,
    sql::{
        Row,
        types::{Value, ValueRef},
    },
    types::EngineError,
};

// A valid canonical row may approach 1 MiB. Eight such rows leave protocol
// overhead below the 16 MiB transport bound and the 32 MiB decode workspace.
const SNAPSHOT_PAGE_ROWS: u32 = 8;

fn source_row(row: &Row<'_>, columns: usize) -> Result<Vec<Value>, EngineError> {
    // Check borrowed text/blob lengths before making an owned row copy. The
    // canonical encoder then checks the exact escaped/base64 JSON size.
    let mut bytes = 0_usize;
    for index in 0..columns {
        let length = match row.get_ref(index).map_err(storage)? {
            ValueRef::Text(value) | ValueRef::Blob(value) => value.len(),
            _ => 8,
        };
        bytes = bytes
            .checked_add(length)
            .filter(|size| *size <= MAX_ROW_BYTES)
            .ok_or_else(|| EngineError::Storage(Box::new(SnapshotError::LimitExceeded)))?;
    }
    (0..columns)
        .map(|index| row.get(index))
        .collect::<crate::sql::Result<Vec<_>>>()
        .map_err(storage)
}

pub(crate) fn visit(
    db: &Db,
    mut visitor: impl FnMut(&CrmTable, &[u8]) -> Result<(), EngineError>,
) -> Result<CrmSequences, EngineError> {
    let transaction = db.conn().unchecked_transaction().map_err(storage)?;
    let occupied: bool = transaction
        .query_row("SELECT EXISTS(SELECT 1 FROM erase_gate)", [], |row| {
            row.get(0)
        })
        .map_err(storage)?;
    if occupied {
        return Err(EngineError::Corrupt);
    }
    let sequences =
        read_crm_sequences(&transaction).map_err(|error| EngineError::Storage(Box::new(error)))?;
    for table in crm_tables() {
        let base = format!(
            "SELECT {} FROM {} WHERE 1",
            table.columns().join(","),
            table.name()
        );
        let keys = table
            .key_columns()
            .iter()
            .map(|key| {
                (
                    *key,
                    table
                        .columns()
                        .iter()
                        .position(|column| column == key)
                        .expect("trusted snapshot key"),
                )
            })
            .collect::<Vec<_>>();
        crate::scan::visit_rows(&transaction, &base, &keys, SNAPSHOT_PAGE_ROWS, |row| {
            let values = source_row(row, table.columns().len())?;
            let encoded =
                encode_row(&values).map_err(|error| EngineError::Storage(Box::new(error)))?;
            visitor(table, &encoded)?;
            Ok(true)
        })?;
    }
    transaction.rollback().map_err(storage)?;
    Ok(sequences)
}
