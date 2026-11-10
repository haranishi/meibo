//! Fixed diagnostic values; never retains provider errors or connection data.

/// The stage reached before a remote transport failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteTransportPhase {
    /// Sending a request and waiting for response headers.
    Request,
    /// Reading the bounded response body after receiving headers.
    ResponseBody,
    /// Validating and decoding the complete response.
    ResponseDecode,
}

/// A bounded failure class, independent of any provider error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteTransportKind {
    /// The existing request deadline expired.
    Timeout,
    /// Connecting failed; this does not distinguish DNS, TCP and TLS failures.
    Connect,
    /// A request or response stream failed for another transport reason.
    Network,
    /// The declared response limit was exceeded.
    Limit,
    /// The provider returned an authentication rejection.
    Authentication,
    /// The provider returned another unsuccessful HTTP status.
    Status,
    /// The response could not be decoded or violated the protocol.
    Invalid,
}

/// First failure metadata for explicit adapter-level diagnosis only.
///
/// This value is never logged automatically or included in HTTP responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteTransportDiagnostic {
    /// Stage reached before failing, without a URL or SQL text.
    pub phase: RemoteTransportPhase,
    /// Fixed classification, without the original exception.
    pub kind: RemoteTransportKind,
}

pub(super) fn classify(error: &reqwest::Error) -> RemoteTransportKind {
    if error.is_timeout() {
        RemoteTransportKind::Timeout
    } else if error.is_connect() {
        RemoteTransportKind::Connect
    } else {
        RemoteTransportKind::Network
    }
}
