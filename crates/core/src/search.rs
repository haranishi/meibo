//! 短い語は部分一致、3文字以上はtrigramで社名を探す。
use crate::sql::params;

use crate::{
    normalize,
    query::{company_row, invalid, storage, unsigned, validate_limit},
    schema::Db,
    types::{Cursor, EngineError, SearchOptions, SearchPage},
};

const FTS_PAGE_SQL: &str = "SELECT c.* FROM name_fts CROSS JOIN companies c ON c.id=name_fts.rowid WHERE name_fts MATCH ? AND name_fts.rowid>? AND (? OR c.status<>5) ORDER BY name_fts.rowid LIMIT ?";

pub(crate) struct NameMatch {
    pub key: String,
    pub phrase: Option<String>,
}

impl NameMatch {
    pub fn new(term: &str) -> Result<Self, EngineError> {
        let (key, length) = normalize::search_term(term)?;
        // FTSの構文解析はNULで終端する。短い語とNULを含む語は部分一致で照合する。
        let phrase = (length >= 3 && !key.contains('\0'))
            .then(|| format!("\"{}\"", key.replace('"', "\"\"")));
        Ok(Self { key, phrase })
    }
}

pub(crate) fn search_name(
    db: &Db,
    term: &str,
    o: &SearchOptions,
) -> Result<SearchPage, EngineError> {
    let _snapshot = db.conn().unchecked_transaction().map_err(storage)?;
    validate_limit(o.limit)?;
    let name = NameMatch::new(term)?;
    let after = o
        .after
        .as_ref()
        .map(|c| {
            c.token
                .strip_prefix("v1:search:")
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| invalid("after", "invalid search cursor"))
        })
        .transpose()?;
    let mut rows;
    if let Some(phrase) = &name.phrase {
        let mut stmt = db.conn().prepare(FTS_PAGE_SQL).map_err(storage)?;
        rows = stmt
            .query_map(
                params![
                    phrase,
                    after.unwrap_or(i64::MIN),
                    o.include_excluded,
                    o.limit + 1
                ],
                |r| company_row(r, db.origin(), &o.fields),
            )
            .map_err(storage)?
            .collect::<crate::sql::Result<Vec<_>>>()
            .map_err(storage)?;
    } else {
        let mut stmt = db.conn().prepare("SELECT * FROM companies WHERE instr(name_key,?)>0 AND id>? AND (? OR status<>5) ORDER BY id LIMIT ?").map_err(storage)?;
        rows = stmt
            .query_map(
                params![
                    name.key,
                    after.unwrap_or(i64::MIN),
                    o.include_excluded,
                    o.limit + 1
                ],
                |r| company_row(r, db.origin(), &o.fields),
            )
            .map_err(storage)?
            .collect::<crate::sql::Result<Vec<_>>>()
            .map_err(storage)?;
    }
    let n = if after.is_some() || !o.with_count {
        None
    } else if rows.len() <= o.limit as usize {
        // limit+1件に届かなければ、候補を最後まで読み切っている。
        Some(rows.len() as u64)
    } else if let Some(phrase) = &name.phrase {
        Some(fts_count(db, phrase, &name.key, o.include_excluded)?)
    } else {
        Some(
            db.conn()
                .query_row(
                    "SELECT count(*) FROM companies WHERE instr(name_key,?)>0 AND (? OR status<>5)",
                    params![name.key, o.include_excluded],
                    unsigned,
                )
                .map_err(storage)?,
        )
    };
    let next = if rows.len() > o.limit as usize {
        rows.pop();
        rows.last().map(|c| Cursor {
            token: format!("v1:search:{}", c.id.0),
        })
    } else {
        None
    };
    Ok(SearchPage { rows, next, n })
}

fn fts_count(db: &Db, phrase: &str, key: &str, include_excluded: bool) -> Result<u64, EngineError> {
    let total: u64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM name_fts WHERE name_fts MATCH ?",
            [phrase],
            unsigned,
        )
        .map_err(storage)?;
    let excluded = if include_excluded {
        0
    } else {
        db.conn()
            .query_row(
                "SELECT count(*) FROM companies INDEXED BY k6 WHERE status=5 AND instr(name_key,?)>0",
                [key],
                unsigned,
            )
            .map_err(storage)?
    };
    total.checked_sub(excluded).ok_or(EngineError::Corrupt)
}

#[cfg(test)]
mod tests;
