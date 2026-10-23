//! 前方移行と、確定後の検査失敗からの復元。
use super::{
    EngineError, Migration, OpenOptions, VERSION, backup, check_integrity, check_version,
    configure, connect, database_error, ensure_space, read_origin, should_check, space_needed,
    storage, suffixed, unix_seconds, validate_header, version,
};
use rusqlite::{Connection, TransactionBehavior};
use std::{io, path::Path};

pub(crate) const MIGRATIONS: &[Migration] = &[];

pub(crate) fn migrate_with(
    path: &Path,
    migrations: &[Migration],
    probe: Option<&dyn super::SpaceProbe>,
    opt: &OpenOptions,
) -> Result<(), EngineError> {
    let check = should_check(path, opt.integrity)?;
    let mut conn = connect(path, opt)?;
    validate_header(&conn)?;
    read_origin(&conn)?;
    let target = migrations
        .iter()
        .map(|m| m.to)
        .max()
        .unwrap_or(VERSION)
        .max(VERSION);
    let from = version(&conn)?;
    if from > target {
        return check_version(from, target);
    }
    if check {
        check_integrity(&conn, true)?;
    }
    if from == target {
        return Ok(());
    }
    let mut chain = Vec::new();
    let mut next = from;
    while next < target {
        let mut candidates = migrations
            .iter()
            .filter(|m| m.from == next && m.to > next && m.to <= target);
        let step = candidates.next().ok_or(EngineError::NeedsMigration)?;
        if candidates.next().is_some() {
            return Err(storage(io::Error::other("ambiguous migration chain")));
        }
        chain.push(step);
        next = step.to;
    }
    ensure_space(
        path,
        space_needed(&conn)?,
        probe.or(opt.space_probe.as_deref()),
    )?;
    configure(&conn, opt)?;
    // BEGIN IMMEDIATE のロック中に別の読み取り接続から複製する。
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    if version(&tx)? != from {
        return Err(storage(io::Error::other(
            "version changed during migration",
        )));
    }
    ensure_space(
        path,
        space_needed(&tx)?,
        probe.or(opt.space_probe.as_deref()),
    )?;
    let before = row_counts(&tx)?;
    let backup_path = suffixed(path, &format!(".bak-v{from}-{}", unix_seconds()?));
    let reader = connect(path, opt)?;
    backup(&reader, &backup_path)?;
    drop(reader);
    let result = (|| {
        for step in chain {
            (step.apply)(&tx)?;
            tx.execute(
                "UPDATE meta SET value=?1 WHERE key='schema_version'",
                [step.to.to_string()],
            )
            .map_err(storage)?;
            tx.pragma_update(None, "user_version", step.to)
                .map_err(storage)?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        tx.rollback().map_err(storage)?;
        verify_restored(&conn, from, &before)?;
        return Err(err);
    }
    if let Err(err) = tx.commit().map_err(storage) {
        // COMMIT が失敗した場合も、複製からの復元を確認する。
        restore(&mut conn, &backup_path, from, &before)?;
        return Err(err);
    }
    let verified = (|| {
        check_integrity(&conn, false)?;
        if row_counts(&conn)? != before || version(&conn)? != target {
            return Err(EngineError::Corrupt);
        }
        let user: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(database_error)?;
        if user != target {
            return Err(EngineError::Corrupt);
        }
        Ok(())
    })();
    if let Err(err) = verified {
        restore(&mut conn, &backup_path, from, &before)?;
        return Err(err);
    }
    Ok(())
}

fn row_counts(conn: &Connection) -> Result<[i64; 5], EngineError> {
    let mut counts = [0; 5];
    for (i, table) in [
        "companies",
        "contacts",
        "activities",
        "lists",
        "list_members",
    ]
    .iter()
    .enumerate()
    {
        counts[i] = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .map_err(database_error)?;
    }
    Ok(counts)
}

fn verify_restored(conn: &Connection, from: u32, before: &[i64; 5]) -> Result<(), EngineError> {
    check_integrity(conn, false)?;
    let user: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(database_error)?;
    if row_counts(conn)? != *before || version(conn)? != from || user != from {
        return Err(EngineError::Corrupt);
    }
    Ok(())
}

fn restore(
    conn: &mut Connection,
    backup_path: &Path,
    from: u32,
    before: &[i64; 5],
) -> Result<(), EngineError> {
    conn.restore(
        rusqlite::MAIN_DB,
        backup_path,
        None::<fn(rusqlite::backup::Progress)>,
    )
    .map_err(database_error)?;
    verify_restored(conn, from, before)
}
