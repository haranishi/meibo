//! 状態と電話の有無の12行を同じ確定で保つ。
use rusqlite::{Connection, Transaction};

#[cfg(test)]
use super::unsigned;
use super::{EngineError, storage};
use crate::types::Status;

pub(crate) fn apply_delta(
    tx: &Transaction<'_>,
    old: Option<(Status, bool)>,
    new: Option<(Status, bool)>,
) -> Result<(), EngineError> {
    if old == new {
        return Ok(());
    }
    for (pair, delta) in [(old, -1), (new, 1)] {
        if let Some((status, phone)) = pair {
            let changed = tx
                .execute(
                    "UPDATE counts SET n=n+?1 WHERE status=?2 AND has_phone=?3 AND n+?1>=0",
                    (delta, status.code(), phone),
                )
                .map_err(storage)?;
            if changed != 1 {
                return Err(EngineError::Corrupt);
            }
        }
    }
    Ok(())
}

pub(crate) fn recount(conn: &Connection) -> Result<(), EngineError> {
    // 呼び出し元の確定にも、単独呼び出しにも原子性を保つ。
    conn.execute_batch("SAVEPOINT meibo_recount")
        .map_err(storage)?;
    let result = (|| {
        conn.execute("DELETE FROM counts", []).map_err(storage)?;
        for s in 0..6 {
            for phone in 0..2 {
                conn.execute(
                    "INSERT INTO counts(status,has_phone,n) SELECT ?1,?2,count(*) FROM companies WHERE status=?1 AND has_phone=?2",
                    (s,phone),
                ).map_err(storage)?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        conn.execute_batch("ROLLBACK TO meibo_recount")
            .map_err(storage)?;
    }
    conn.execute_batch("RELEASE meibo_recount")
        .map_err(storage)?;
    result
}

pub(crate) fn matches_recount(conn: &Connection) -> bool {
    conn.query_row(
        "WITH expected(status,has_phone,n) AS (
          SELECT s.value,p.value,(SELECT count(*) FROM companies WHERE status=s.value AND has_phone=p.value)
          FROM (SELECT 0 value UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5) s
          CROSS JOIN (SELECT 0 value UNION ALL SELECT 1) p)
        SELECT NOT EXISTS(SELECT * FROM counts EXCEPT SELECT * FROM expected)
          AND NOT EXISTS(SELECT * FROM expected EXCEPT SELECT * FROM counts)
          AND NOT EXISTS(SELECT 1 FROM companies WHERE status NOT BETWEEN 0 AND 5)",
        [], |r| r.get(0),
    ).unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn total(conn: &Connection) -> Result<u64, EngineError> {
    conn.query_row("SELECT coalesce(sum(n),0) FROM counts", [], unsigned)
        .map_err(storage)
}

#[cfg(test)]
pub(crate) fn by_status(conn: &Connection, status: Status) -> Result<u64, EngineError> {
    conn.query_row(
        "SELECT coalesce(sum(n),0) FROM counts WHERE status=?1",
        [status.code()],
        unsigned,
    )
    .map_err(storage)
}

#[cfg(test)]
pub(crate) fn by_phone_excluding_excluded(
    conn: &Connection,
    has_phone: bool,
) -> Result<u64, EngineError> {
    conn.query_row(
        "SELECT coalesce(sum(n),0) FROM counts WHERE has_phone=?1 AND status<>5",
        [has_phone],
        unsigned,
    )
    .map_err(storage)
}
