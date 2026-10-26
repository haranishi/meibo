//! Read-only import of a verified national corporate directory catalog.
use meibo_core::{
    Engine,
    public_data::{PublicCompanyInput, PublicImportReport},
    types::DataOrigin,
};
use rusqlite::{Connection, OpenFlags};
use std::{fmt, path::Path};

/// Import failure without source rows, personal values or private paths.
#[derive(Debug)]
pub struct NtaImportError {
    /// One-based source row, or zero for preflight failure.
    pub row: u64,
    /// Changes committed before the failed batch.
    pub report: PublicImportReport,
    reason: &'static str,
}
impl fmt::Display for NtaImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}; row={}; committed inserted={} updated={} skipped={}",
            self.reason, self.row, self.report.inserted, self.report.updated, self.report.skipped
        )
    }
}
impl std::error::Error for NtaImportError {}

fn fail(row: u64, report: PublicImportReport, reason: &'static str) -> NtaImportError {
    NtaImportError {
        row,
        report,
        reason,
    }
}

/// Import only allowlisted NTA fields, refreshing by corporate number in bounded batches.
/// The catalog is opened read-only; its signatures must have been verified upstream.
/// # Errors
/// Invalid origin, unverified/unknown sources, malformed rows or storage failure.
pub fn sync_nta(
    engine: &mut Engine,
    path: &Path,
    limit: Option<u64>,
) -> Result<PublicImportReport, NtaImportError> {
    let empty = PublicImportReport::default();
    if engine.origin() != DataOrigin::Real
        || limit == Some(0)
        || engine.path().is_some_and(|p| p == path)
    {
        return Err(fail(
            0,
            empty,
            "a separate real CRM destination and positive limit are required",
        ));
    }
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| fail(0, empty, "cannot open source catalog read-only"))?;
    source
        .execute_batch("PRAGMA query_only=ON; BEGIN")
        .map_err(|_| fail(0, empty, "cannot start source snapshot"))?;
    let invalid:bool=source.query_row(
        "SELECT EXISTS(SELECT 1 FROM companies c LEFT JOIN source_batches s ON s.id=c.source_batch_id WHERE NOT (s.id IS NOT NULL AND COALESCE(s.source_type,'') IN ('nta_monthly_all','nta_daily') AND COALESCE(s.state,'')='complete' AND COALESCE(s.signature_verified,0)=1 AND COALESCE(s.signature_fingerprint,'')<>'' AND COALESCE(s.as_of,'')<>''))",[],|r|r.get(0))
        .map_err(|_|fail(0,empty,"unsupported source catalog schema"))?;
    if invalid {
        return Err(fail(0, empty, "unverified or non-NTA source rejected"));
    }
    let daily:bool=source.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='daily_changes')",[],|r|r.get(0))
        .map_err(|_|fail(0,empty,"cannot inspect source catalog"))?;
    if daily {
        let invalid:bool=source.query_row("SELECT EXISTS(SELECT 1 FROM daily_changes d LEFT JOIN daily_parts p ON p.as_of=d.as_of AND p.part_number=d.part_number WHERE p.as_of IS NULL OR COALESCE(p.signature_fingerprint,'')='' OR length(COALESCE(p.csv_sha256,''))<>64 OR d.is_deleted IS NULL OR d.is_deleted NOT IN (0,1))",[],|r|r.get(0))
            .map_err(|_|fail(0,empty,"unsupported or unverified daily source"))?;
        if invalid {
            return Err(fail(0, empty, "unverified daily source rejected"));
        }
    }
    let sql = if daily {
        "SELECT c.corporate_number,c.legal_name,c.prefecture_code,s.as_of FROM companies c JOIN source_batches s ON s.id=c.source_batch_id WHERE NOT EXISTS(SELECT 1 FROM daily_changes d WHERE d.corporate_number=c.corporate_number) UNION ALL SELECT d.corporate_number,d.legal_name,d.prefecture_code,d.as_of FROM daily_changes d WHERE d.is_deleted=0 ORDER BY 1 LIMIT ?1"
    } else {
        "SELECT c.corporate_number,c.legal_name,c.prefecture_code,s.as_of FROM companies c JOIN source_batches s ON s.id=c.source_batch_id ORDER BY c.corporate_number LIMIT ?1"
    };
    let mut stmt = source
        .prepare(sql)
        .map_err(|_| fail(0, empty, "unsupported source catalog schema"))?;
    let mut cursor = stmt
        .query([limit
            .and_then(|n| i64::try_from(n).ok())
            .unwrap_or(i64::MAX)])
        .map_err(|_| fail(0, empty, "cannot read source catalog"))?;
    let mut report = empty;
    let mut batch = Vec::with_capacity(500);
    let mut row = 0_u64;
    while let Some(record) = cursor
        .next()
        .map_err(|_| fail(row + 1, report, "cannot read source row"))?
    {
        row += 1;
        let input = (|| -> rusqlite::Result<PublicCompanyInput> {
            let prefecture: String = record.get(2)?;
            Ok(PublicCompanyInput {
                corporate_number: record.get(0)?,
                name: record.get(1)?,
                prefecture: prefecture.parse().unwrap_or(0),
                source_as_of: record.get(3)?,
            })
        })()
        .map_err(|_| fail(row, report, "invalid source row"))?;
        batch.push(input);
        if batch.len() == 500 {
            commit(engine, &mut batch, &mut report, row)?;
        }
    }
    if !batch.is_empty() {
        commit(engine, &mut batch, &mut report, row)?;
    }
    Ok(report)
}

fn commit(
    engine: &mut Engine,
    batch: &mut Vec<PublicCompanyInput>,
    report: &mut PublicImportReport,
    row: u64,
) -> Result<(), NtaImportError> {
    let done = engine
        .upsert_public_companies(batch)
        .map_err(|_| fail(row, *report, "source batch was not committed"))?;
    report.inserted += done.inserted;
    report.updated += done.updated;
    report.skipped += done.skipped;
    batch.clear();
    Ok(())
}
