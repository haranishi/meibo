use crate::Error;

/// Stable adapter outcomes, separate from provider SQL constraint failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteErrorKind {
    /// A sent operation may have completed and must not be sent again.
    OutcomeUnknown,
    /// An earlier failure makes every further database request unsafe.
    ConnectionPoisoned,
    /// The selected backend does not support this operation.
    Unsupported,
}

impl RemoteErrorKind {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::OutcomeUnknown => "remote database outcome unknown",
            Self::ConnectionPoisoned => "remote database connection poisoned",
            Self::Unsupported => "remote database operation unsupported",
        }
    }

    pub(super) const fn code(self) -> i32 {
        match self {
            Self::OutcomeUnknown => rusqlite::ffi::SQLITE_IOERR,
            Self::ConnectionPoisoned | Self::Unsupported => rusqlite::ffi::SQLITE_MISUSE,
        }
    }

    pub(crate) fn error(self) -> Error {
        Error::SqliteFailure(
            rusqlite::ffi::Error::new(self.code()),
            Some(self.message().to_owned()),
        )
    }
}

/// Classify only fixed adapter messages with their reserved SQLite error code.
/// Provider messages are never retained and cannot masquerade as these outcomes.
#[must_use]
pub fn remote_error_kind(error: &Error) -> Option<RemoteErrorKind> {
    let Error::SqliteFailure(error, Some(message)) = error else {
        return None;
    };
    [
        RemoteErrorKind::OutcomeUnknown,
        RemoteErrorKind::ConnectionPoisoned,
        RemoteErrorKind::Unsupported,
    ]
    .into_iter()
    .find(|kind| error.extended_code == kind.code() && message == kind.message())
}
