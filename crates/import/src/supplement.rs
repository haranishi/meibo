use std::{error::Error, fmt, io::Read};

use meibo_core::{
    Engine,
    company_contacts::{CompanyContactUpdate, ContactFillReport},
    types::{DataOrigin, Industry},
};

use crate::{
    Encoding,
    csv::{CsvReader, ReadError},
    import::engine_reason,
    row::decode,
};

const COLUMNS: [&str; 5] = ["source", "corporate_number", "phone", "website", "industry"];
const BATCH: usize = 500;

/// An invalid input or failed batch, including only counts known to be committed.
#[derive(Debug)]
pub struct ContactCsvError {
    /// Logical CSV record (the header is record 1).
    pub row: u64,
    /// Results from finished transactions.
    pub report: ContactFillReport,
    reason: &'static str,
}

impl ContactCsvError {
    fn at(row: u64, report: ContactFillReport, reason: &'static str) -> Self {
        Self {
            row,
            report,
            reason,
        }
    }
    fn csv(e: &ReadError, report: ContactFillReport) -> Self {
        Self::at(e.row, report, e.reason)
    }
}

impl fmt::Display for ContactCsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CSV record {}: {}; committed updated={}, skipped={}",
            self.row, self.reason, self.report.updated, self.report.skipped
        )
    }
}
impl Error for ContactCsvError {}

/// Fill existing companies from an operator-owned contact list (or a synthetic fixture).
/// Validates strict UTF-8/Windows-31J records and commits at most 500 per transaction.
///
/// # Errors
/// Rejects unknown sources, malformed records, an origin mismatch or a failed database operation.
pub fn supplement_contacts(
    engine: &mut Engine,
    reader: impl Read,
    encoding: Encoding,
) -> Result<ContactFillReport, ContactCsvError> {
    let mut total = ContactFillReport::default();
    let mut csv = CsvReader::new(reader, encoding == Encoding::Utf8)
        .map_err(|e| ContactCsvError::csv(&e, total))?;
    let header = csv
        .next()
        .map_err(|e| ContactCsvError::csv(&e, total))?
        .ok_or_else(|| ContactCsvError::at(1, total, "missing header"))?;
    let labels =
        decode(header.cells, encoding).map_err(|reason| ContactCsvError::at(1, total, reason))?;
    if !labels.iter().map(String::as_str).eq(COLUMNS) {
        return Err(ContactCsvError::at(
            1,
            total,
            "expected source,corporate_number,phone,website,industry",
        ));
    }
    let origin = engine.origin();
    let mut batch = Vec::with_capacity(BATCH);
    let mut start = 2;
    while let Some(record) = csv.next().map_err(|e| ContactCsvError::csv(&e, total))? {
        let at = |reason| ContactCsvError::at(record.row, total, reason);
        let cells = decode(record.cells, encoding).map_err(at)?;
        let [source, number, phone, website, industry] = cells.as_slice() else {
            return Err(at("expected five columns"));
        };
        let input_origin = match source.as_str() {
            "first_party" => DataOrigin::Real,
            "synthetic" => DataOrigin::Synthetic,
            _ => return Err(at("source must be first_party or synthetic")),
        };
        if input_origin != origin {
            return Err(at("contact source differs from database origin"));
        }
        let industry = industry.trim();
        let industry = if industry.is_empty() {
            None
        } else {
            let code = match industry.as_bytes() {
                [letter @ b'A'..=b'T'] => Some(letter - b'A' + 1),
                _ => industry.parse::<u8>().ok(),
            };
            Some(
                code.and_then(Industry::from_code)
                    .ok_or_else(|| at("industry must be A..T or 1..20"))?,
            )
        };
        batch.push(
            CompanyContactUpdate::new(number, phone, website, industry)
                .map_err(|e| at(engine_reason(&e)))?,
        );
        if batch.len() == BATCH {
            commit(engine, origin, &batch, start, &mut total)?;
            batch.clear();
            start = record.row + 1;
        }
    }
    if !batch.is_empty() {
        commit(engine, origin, &batch, start, &mut total)?;
    }
    Ok(total)
}

fn commit(
    engine: &mut Engine,
    origin: DataOrigin,
    rows: &[CompanyContactUpdate],
    start: u64,
    total: &mut ContactFillReport,
) -> Result<(), ContactCsvError> {
    let done = engine
        .fill_company_contacts(origin, rows)
        .map_err(|e| ContactCsvError::at(start, *total, engine_reason(&e)))?;
    total.updated += done.updated;
    total.skipped += done.skipped;
    Ok(())
}
