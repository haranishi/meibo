use std::{error::Error, fmt, io::Read};

use meibo_core::{
    Engine,
    types::{CompanyInput, EngineError},
};

use crate::{
    Encoding, ImportOptions, ImportReport,
    csv::CsvReader,
    row::{Headers, decode},
};

const MAX_BATCH_ROWS: usize = 500;
const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// A CSV failure with its logical record number and already committed counts.
/// The header is record 1. Input values and underlying I/O messages are omitted.
#[derive(Debug)]
pub struct ImportError {
    /// Logical CSV record that failed, including a header at record 1.
    pub row: u64,
    /// Changes committed before the failing batch.
    pub report: ImportReport,
    reason: String,
}

impl ImportError {
    fn new(row: u64, report: ImportReport, reason: impl Into<String>) -> Self {
        Self {
            row,
            report,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CSV record {}: {}; committed {}; earlier batches remain saved",
            self.row, self.reason, self.report
        )
    }
}

impl Error for ImportError {}

/// Import strict CSV in atomic batches of at most 500 rows or 8 MiB of strings.
/// A failure discards the pending batch; earlier committed batches remain saved.
///
/// # Errors
/// Invalid CSV, encoding, headers, values, storage failures or rejected duplicates.
pub fn import_csv(
    engine: &mut Engine,
    reader: impl Read,
    options: ImportOptions,
) -> Result<ImportReport, ImportError> {
    let mut report = ImportReport::default();
    let mut csv = CsvReader::new(reader, options.encoding == Encoding::Utf8)
        .map_err(|error| ImportError::new(error.row, report, error.reason))?;
    let header = csv
        .next()
        .map_err(|error| ImportError::new(error.row, report, error.reason))?
        .ok_or_else(|| ImportError::new(1, report, "missing header"))?;
    let cells = decode(header.cells, options.encoding)
        .map_err(|reason| ImportError::new(1, report, reason))?;
    let headers = Headers::new(&cells).map_err(|reason| ImportError::new(1, report, reason))?;
    let mut batch = Vec::new();
    let mut rows = Vec::new();
    let mut batch_bytes = 0;
    while let Some(record) = csv
        .next()
        .map_err(|error| ImportError::new(error.row, report, error.reason))?
    {
        let cells = decode(record.cells, options.encoding)
            .map_err(|reason| ImportError::new(record.row, report, reason))?;
        let bytes: usize = cells.iter().map(String::len).sum();
        let company = headers
            .company(&cells)
            .map_err(|reason| ImportError::new(record.row, report, reason))?;
        if batch_bytes + bytes > MAX_BATCH_BYTES && !batch.is_empty() {
            commit(engine, &batch, &rows, options, &mut report)?;
            batch.clear();
            rows.clear();
            batch_bytes = 0;
        }
        batch_bytes += bytes;
        batch.push(company);
        rows.push(record.row);
        if batch.len() == MAX_BATCH_ROWS {
            commit(engine, &batch, &rows, options, &mut report)?;
            batch.clear();
            rows.clear();
            batch_bytes = 0;
        }
    }
    if !batch.is_empty() {
        commit(engine, &batch, &rows, options, &mut report)?;
    }
    Ok(report)
}

fn commit(
    engine: &mut Engine,
    batch: &[CompanyInput],
    rows: &[u64],
    options: ImportOptions,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let result = engine
        .import_companies(batch, options.duplicates)
        .map_err(|error| {
            let row = error
                .row
                .checked_sub(1)
                .and_then(|index| rows.get(index))
                .copied()
                .unwrap_or(rows[0]);
            ImportError::new(row, *report, engine_reason(&error.error))
        })?;
    report.inserted += result.inserted;
    report.skipped += result.skipped;
    Ok(())
}

pub(crate) fn engine_reason(error: &EngineError) -> &'static str {
    match error {
        EngineError::InvalidInput { field, .. } => match field.as_str() {
            "prefecture" => "prefecture must be an integer from 1 to 47",
            "municipality" => "municipality must contain five digits matching prefecture",
            "source_as_of" => "source_as_of must be a valid YYYY-MM-DD date",
            "name" => "name contains an invalid character",
            "phone" => "phone contains an invalid character",
            "website" => "website contains an invalid character",
            "source_id" => "source_id contains an invalid character",
            _ => "invalid company field (core validation)",
        },
        EngineError::EmptyName => "name must not be empty",
        EngineError::InvalidCorporateNumber { .. } => "invalid corporate number",
        EngineError::DuplicateCorporateNumber { .. } => "duplicate corporate number",
        EngineError::OriginMismatch => "data origin differs from database",
        EngineError::InsufficientSpace => "insufficient storage space",
        _ => "database operation failed",
    }
}
