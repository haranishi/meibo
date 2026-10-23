use std::{error::Error, fmt, io::Write};

use encoding_rs::SHIFT_JIS;
use meibo_core::{
    Engine,
    types::{Company, DataOrigin, EngineError, Field, Filters},
};

use crate::{Encoding, ExportOptions, import::engine_reason, row::COLUMNS};

/// Export failed at a logical CSV record. No input values or I/O messages are exposed.
#[derive(Debug)]
pub struct ExportError {
    /// Header is record 1, followed by exported companies.
    pub row: u64,
    reason: &'static str,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CSV record {}: {}", self.row, self.reason)
    }
}

impl Error for ExportError {}

/// Export a single database snapshot, writing UTF-8 BOM/CRLF or strict `Shift_JIS`.
/// Erased rows are omitted. Sole proprietors require `include_personal`.
/// Formula protection is retained if the resulting CSV is imported again.
/// The caller is responsible for atomically publishing the destination file.
///
/// # Errors
/// Storage or writer failures, or characters that `Shift_JIS` cannot represent.
pub fn export_csv(
    engine: &Engine,
    mut writer: impl Write,
    options: ExportOptions,
) -> Result<u64, ExportError> {
    if options.encoding == Encoding::Utf8 {
        writer
            .write_all(&[0xef, 0xbb, 0xbf])
            .map_err(|_| ExportError {
                row: 1,
                reason: "output I/O failed",
            })?;
    }
    write_record(
        &mut writer,
        &COLUMNS.map(str::to_owned),
        options.encoding,
        1,
    )?;
    let filters = Filters {
        include_excluded: options.include_excluded,
        ..Filters::default()
    };
    let fields = if options.include_personal {
        vec![Field::Name, Field::Phone, Field::Website]
    } else {
        vec![]
    };
    let mut count = 0;
    let mut failure = None;
    let visited = engine.visit_companies(&filters, &fields, |company| {
        if company.erased_at.is_some() || (company.is_sole_proprietor && !options.include_personal)
        {
            return Ok(());
        }
        match write_record(
            &mut writer,
            &company_cells(company),
            options.encoding,
            count + 2,
        ) {
            Ok(()) => {
                count += 1;
                Ok(())
            }
            Err(error) => {
                failure = Some(error);
                Err(EngineError::Storage(Box::new(std::io::Error::other(
                    "CSV writer stopped",
                ))))
            }
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    visited.map_err(|error| ExportError {
        row: count + 2,
        reason: engine_reason(&error),
    })?;
    writer.flush().map_err(|_| ExportError {
        row: count + 2,
        reason: "output I/O failed",
    })?;
    Ok(count)
}

fn company_cells(company: Company) -> [String; 13] {
    let mut cells = [
        match company.origin {
            DataOrigin::Synthetic => "synthetic",
            DataOrigin::Real => "real",
        }
        .into(),
        company.corporate_number.unwrap_or_default(),
        company.name.unwrap_or_default(),
        company.prefecture.to_string(),
        company.municipality.unwrap_or_default(),
        company
            .industry
            .map(|value| value.code().to_string())
            .unwrap_or_default(),
        company.phone.unwrap_or_default(),
        company.website.unwrap_or_default(),
        company
            .owner
            .map(|value| value.0.to_string())
            .unwrap_or_default(),
        company.status.code().to_string(),
        company.is_sole_proprietor.to_string(),
        company.source_id.unwrap_or_default(),
        company.source_as_of.unwrap_or_default(),
    ];
    for index in [1, 2, 4, 6, 7, 11, 12] {
        let text = &mut cells[index];
        if text.starts_with(['\t', '\r', '\n'])
            || text.trim_start().starts_with(['=', '+', '-', '@'])
        {
            text.insert(0, '\'');
        }
    }
    cells
}

fn write_record(
    writer: &mut impl Write,
    cells: &[String; 13],
    encoding: Encoding,
    row: u64,
) -> Result<(), ExportError> {
    let mut record = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index != 0 {
            record.push(',');
        }
        if cell.contains([',', '"', '\r', '\n']) {
            record.push('"');
            for character in cell.chars() {
                if character == '"' {
                    record.push('"');
                }
                record.push(character);
            }
            record.push('"');
        } else {
            record.push_str(cell);
        }
    }
    record.push_str("\r\n");
    let result = match encoding {
        Encoding::Utf8 => writer.write_all(record.as_bytes()),
        Encoding::ShiftJis => {
            let (bytes, _, replacements) = SHIFT_JIS.encode(&record);
            if replacements {
                return Err(ExportError {
                    row,
                    reason: "character cannot be represented in Shift_JIS",
                });
            }
            writer.write_all(&bytes)
        }
    };
    result.map_err(|_| ExportError {
        row,
        reason: "output I/O failed",
    })
}
