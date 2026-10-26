//! Bounded, strict CSV conversion for the local company engine.
mod csv;
mod export;
mod import;
mod nta;
mod row;

use std::fmt;

use meibo_core::DuplicatePolicy;

pub use export::{ExportError, export_csv};
pub use import::{ImportError, import_csv};
pub use nta::{NtaImportError, sync_nta};

/// Explicit input/output character encoding. No encoding is guessed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8. Input accepts an optional BOM; output always includes one.
    #[default]
    Utf8,
    /// Windows-31J, decoded without replacement characters.
    ShiftJis,
}

/// Options for company import.
#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    /// Encoding of the input bytes.
    pub encoding: Encoding,
    /// How corporate-number duplicates are handled.
    pub duplicates: DuplicatePolicy,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            encoding: Encoding::Utf8,
            duplicates: DuplicatePolicy::Error,
        }
    }
}

/// Counts of committed changes; uncommitted batches are never counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Companies inserted by committed batches.
    pub inserted: u64,
    /// Duplicates skipped by committed batches.
    pub skipped: u64,
}

impl fmt::Display for ImportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "inserted={}, skipped={}", self.inserted, self.skipped)
    }
}

/// Options for exporting company fields. Contacts, activity and notes are omitted.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    /// Output encoding.
    pub encoding: Encoding,
    /// Include sole proprietors and their name, telephone and website.
    pub include_personal: bool,
    /// Include excluded companies. Erased companies are always omitted.
    pub include_excluded: bool,
}
