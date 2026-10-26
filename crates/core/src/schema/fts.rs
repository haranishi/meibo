//! 外部内容表の全文索引を明示的に更新する。
use crate::sql::{Connection, Transaction};

use super::{EngineError, storage};

pub(crate) fn insert(tx: &Transaction<'_>, id: i64, key: &str) -> Result<(), EngineError> {
    tx.execute(
        "INSERT INTO name_fts(rowid,name_key) VALUES(?1,?2)",
        (id, key),
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) fn delete(tx: &Transaction<'_>, id: i64, old_key: &str) -> Result<(), EngineError> {
    tx.execute(
        "INSERT INTO name_fts(name_fts,rowid,name_key) VALUES('delete',?1,?2)",
        (id, old_key),
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) fn rebuild(conn: &Connection) -> Result<(), EngineError> {
    conn.execute("INSERT INTO name_fts(name_fts) VALUES('rebuild')", [])
        .map_err(storage)?;
    Ok(())
}
