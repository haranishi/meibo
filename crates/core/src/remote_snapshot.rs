//! Bounded, typed snapshot rows and the trusted CRM schema profile.

use std::fmt;

mod codec;
mod reader;
mod registry;
mod sequence;

pub use codec::{decode_row, encode_row};
pub use registry::{CrmTable, crm_table, crm_tables};
pub use sequence::{CrmSequences, decode_crm_sequences, encode_crm_sequences, read_crm_sequences};

/// Maximum encoded UTF-8 bytes in a snapshot row, excluding its newline.
pub const MAX_ROW_BYTES: usize = 1024 * 1024;
/// Maximum values accepted by the shared row codec.
pub const MAX_COLUMNS: usize = 2_000;
/// Maximum decoded working bytes reserved by one row operation.
pub const MAX_DECODE_WORKSPACE: usize = 32 * 1024 * 1024;

/// A fixed diagnostic which never contains snapshot input or SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    /// The row is not the canonical, typed version-one format.
    InvalidRow,
    /// An encoded row, column count or decode workspace exceeds its bound.
    LimitExceeded,
    /// The sequence profile or source state is invalid.
    InvalidSequence,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::InvalidRow => "invalid snapshot row",
            Self::LimitExceeded => "snapshot row limit exceeded",
            Self::InvalidSequence => "invalid snapshot sequence",
        })
    }
}
impl std::error::Error for SnapshotError {}

/// Snapshot operations return fixed, redacted errors.
pub type Result<T> = std::result::Result<T, SnapshotError>;

pub(crate) use reader::visit;

#[cfg(test)]
mod tests;
