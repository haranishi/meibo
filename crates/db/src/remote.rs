//! Bounded Hrana v2 transport and complete non-transactional SQL operations.
mod budget;
mod error_kind;
mod options;
mod sql;
mod state;

pub use error_kind::{RemoteErrorKind, remote_error_kind};
pub use options::RemoteOptions;

use libsql_hrana::proto::{
    self, CloseStreamReq, DescribeStreamReq, ExecuteStreamReq, PipelineReqBody, PipelineRespBody,
    SequenceStreamReq, Stmt, StmtResult, StoreSqlStreamReq, StreamRequest, StreamResponse,
    StreamResult,
};
use reqwest::{Url, header::HeaderValue};
use std::{
    cell::{Cell, RefCell},
    fmt,
    io::Write,
};

use crate::{Error, Params, Result, Row, Statement, Value};

/// A remote SQL connection with bounded responses and private credentials.
///
/// Autocommit operations use fresh foreign-key-verified streams. An explicit
/// wrapper transaction owns one shared stream across all original connection
/// and prepared statement operations, then closes it exactly once.
pub struct RemoteConnection {
    transport: Transport,
    poisoned: Cell<bool>,
    last_insert_rowid: Cell<i64>,
    active: RefCell<Option<state::Stream>>,
}

struct Transport {
    http: reqwest::Client,
    pipeline: Url,
    authorization: HeaderValue,
    timeout: std::time::Duration,
    response_limit: usize,
}

impl RemoteConnection {
    /// Construct a verified HTTPS connection without making a network request.
    ///
    /// # Errors
    /// Returns a sanitized configuration or HTTP client initialization error.
    pub fn connect(options: RemoteOptions) -> Result<Self> {
        let timeout = options.timeout();
        let response_limit = options.response_limit();
        let (endpoint, authorization) = options.validated()?;
        let pipeline = endpoint
            .join("v2/pipeline")
            .map_err(|_| failure("invalid remote database configuration"))?;
        let http = reqwest::Client::builder()
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            // Hyper can retry a canceled checkout on a reused HTTP/1 socket
            // independently of reqwest's policy. Disable idle socket reuse.
            .pool_max_idle_per_host(0)
            .timeout(timeout)
            .tls_version_min(reqwest::tls::Version::TLS_1_2)
            .tls_danger_accept_invalid_hostnames(false)
            .tls_danger_accept_invalid_certs(false)
            .connection_verbose(false)
            .build()
            .map_err(|_| failure("remote database client unavailable"))?;
        Ok(Self {
            transport: Transport {
                http,
                pipeline,
                authorization,
                timeout,
                response_limit,
            },
            poisoned: Cell::new(false),
            last_insert_rowid: Cell::new(0),
            active: RefCell::new(None),
        })
    }

    /// Whether a transport or protocol failure has invalidated this connection.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.get()
    }

    /// Return the most recent inserted id from its original write stream.
    #[must_use]
    pub fn last_insert_rowid(&self) -> i64 {
        self.last_insert_rowid.get()
    }

    pub(crate) fn is_autocommit(&self) -> bool {
        !self.poisoned.get() && self.active.borrow().is_none()
    }

    fn ensure_usable(&self) -> Result<()> {
        if self.poisoned.get() {
            Err(RemoteErrorKind::ConnectionPoisoned.error())
        } else {
            Ok(())
        }
    }

    /// Describe trusted SQL for repeated typed execution.
    ///
    /// # Errors
    /// Returns sanitized SQL/transport errors or rejects an invalid connection.
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        self.ensure_usable()?;
        sql::inspect(sql, self.active.borrow().is_some())?;
        let response = self.request(
            StreamRequest::Describe(DescribeStreamReq {
                sql: Some(sql.to_owned()),
                ..DescribeStreamReq::default()
            }),
            true,
        )?;
        match response {
            StreamResponse::Describe(response) => Ok(Statement::from_remote(
                self,
                sql.to_owned(),
                response.result.params.len(),
                response.result.cols.len(),
                response.result.is_readonly,
            )),
            _ => self.invalid_response(),
        }
    }

    /// Prepare reusable remote SQL. Its text is held by this statement only.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::prepare`].
    pub fn prepare_cached(&self, sql: &str) -> Result<Statement<'_>> {
        self.prepare(sql)
    }

    /// Execute a single statement, returning its affected row count.
    ///
    /// # Errors
    /// Returns SQL, parameter, bounded transport or unexpected-result errors.
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<usize> {
        // Convert before making any network request, including Describe.
        let values = params.into_values()?;
        self.prepare(sql)?.execute(crate::params_from_iter(values))
    }

    /// Execute a complete trusted SQL script, including trigger definitions.
    ///
    /// # Errors
    /// Returns a sanitized SQL or bounded transport error. Sequence is sent as
    /// one script; its statements are never split on semicolons.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.ensure_usable()?;
        let effects = sql::inspect(sql, self.active.borrow().is_some())?;
        let (response, rowid) = self.request_with_rowid(
            StreamRequest::Sequence(SequenceStreamReq {
                sql: Some(sql.to_owned()),
                ..SequenceStreamReq::default()
            }),
            effects.inserts,
            false,
        )?;
        match response {
            StreamResponse::Sequence(_) => {
                if let Some(id) = rowid {
                    self.last_insert_rowid.set(id);
                }
                Ok(())
            }
            _ => self.invalid_response(),
        }
    }

    /// Convert the first result row, ignoring subsequent rows.
    ///
    /// # Errors
    /// Returns parameter/SQL/transport/conversion errors or `QueryReturnedNoRows`.
    pub fn query_row<T, P, F>(&self, sql: &str, params: P, map: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let values = params.into_values()?;
        self.prepare(sql)?
            .query_row(crate::params_from_iter(values), map)
    }

    pub(crate) fn execute_values(
        &self,
        sql: &str,
        values: Vec<Value>,
        want_rows: bool,
        read_only: bool,
    ) -> Result<StmtResult> {
        self.ensure_usable()?;
        let effects = sql::inspect(sql, self.active.borrow().is_some())?;
        let mut stmt = Stmt::new(sql, want_rows);
        stmt.args = values.into_iter().map(encode_value).collect();
        match self.request(StreamRequest::Execute(ExecuteStreamReq { stmt }), read_only)? {
            StreamResponse::Execute(response) => {
                if response.result.rows.iter().any(|row| {
                    row.values.len() != response.result.cols.len()
                        || row.values.iter().any(|value| {
                            matches!(value, proto::Value::None)
                                || matches!(value, proto::Value::Float { value } if !value.is_finite())
                        })
                }) {
                    return self.invalid_response();
                }
                if effects.inserts
                    && response.result.affected_row_count > 0
                    && let Some(id) = response.result.last_insert_rowid
                {
                    self.last_insert_rowid.set(id);
                }
                Ok(response.result)
            }
            _ => self.invalid_response(),
        }
    }

    fn invalid_response<T>(&self) -> Result<T> {
        self.poisoned.set(true);
        Err(RemoteErrorKind::OutcomeUnknown.error())
    }

    fn pipeline(
        &self,
        url: &Url,
        baton: Option<String>,
        requests: Vec<StreamRequest>,
        closing: bool,
    ) -> Result<PipelineRespBody> {
        self.ensure_usable()?;
        let body = PipelineReqBody { baton, requests };
        let result = self.transport.pipeline(url, &body, closing);
        if result.is_err() {
            self.poisoned.set(true);
        }
        result
    }
}

impl Transport {
    fn pipeline(
        &self,
        url: &Url,
        body: &PipelineReqBody,
        closing: bool,
    ) -> Result<PipelineRespBody> {
        let mut encoded = BoundedWriter(Vec::new());
        serde_json::to_writer(&mut encoded, body)
            .map_err(|_| failure("remote database request exceeds limit"))?;
        let runtime =
            crate::runtime().map_err(|_| failure("remote database runtime unavailable"))?;
        let run = || runtime.block_on(self.receive(url, encoded.0));
        // Synchronous domain calls can occur inside an async host. A separate
        // blocking thread avoids nesting block_on inside that host's runtime;
        // IO/timers still use the one process-wide database runtime.
        let response = if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                let bridge = std::thread::Builder::new()
                    .name("meibo-db-bridge".into())
                    .spawn_scoped(scope, run)
                    .map_err(|_| failure("remote database request failed"))?;
                bridge
                    .join()
                    .map_err(|_| failure("remote database request failed"))?
            })?
        } else {
            run()?
        };
        if response.results.len() != body.requests.len()
            || (closing && response.baton.is_some())
            || (!closing
                && !response
                    .baton
                    .as_ref()
                    .is_some_and(|baton| !baton.is_empty() && baton.len() <= 8192))
        {
            return Err(failure("invalid remote database response"));
        }
        // Validate the server origin before any token-bearing follow-up.
        self.next_pipeline(response.base_url.as_deref())?;
        for (request, result) in body.requests.iter().zip(&response.results) {
            match result {
                StreamResult::Ok { response } if matches_response(request, response) => {}
                StreamResult::Error { .. } => {}
                _ => return Err(failure("invalid remote database response")),
            }
        }
        Ok(response)
    }

    fn next_pipeline(&self, base_url: Option<&str>) -> Result<Url> {
        let Some(base) = base_url else {
            return Ok(self.pipeline.clone());
        };
        if !options::safe_raw_url(base) {
            return Err(failure("invalid remote database response"));
        }
        let mut base = Url::parse(base).map_err(|_| failure("invalid remote database response"))?;
        if !options::safe_url(&base)
            || !matches!(base.scheme(), "http" | "https")
            || base.origin() != self.pipeline.origin()
        {
            return Err(failure("invalid remote database response"));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        base.join("v2/pipeline")
            .map_err(|_| failure("invalid remote database response"))
    }

    async fn receive(&self, url: &Url, body: Vec<u8>) -> Result<PipelineRespBody> {
        let started = std::time::Instant::now();
        // The outer deadline also covers processing between received chunks.
        tokio::time::timeout(self.timeout, async {
            let mut response = self
                .http
                .post(url.clone())
                .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|_| failure("remote database request failed"))?;
            let status = response.status();
            if response
                .content_length()
                .is_some_and(|length| length > self.response_limit as u64)
            {
                return Err(failure("remote database response exceeds limit"));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| failure("remote database response interrupted"))?
            {
                if chunk.len() > self.response_limit.saturating_sub(bytes.len()) {
                    return Err(failure("remote database response exceeds limit"));
                }
                reserve_bounded(&mut bytes, chunk.len(), self.response_limit)
                    .map_err(|_| failure("remote database response exceeds limit"))?;
                bytes.extend_from_slice(&chunk);
            }
            if !status.is_success() {
                return Err(failure(
                    if status == reqwest::StatusCode::UNAUTHORIZED
                        || status == reqwest::StatusCode::FORBIDDEN
                    {
                        "remote database authentication failed"
                    } else {
                        "remote database request rejected"
                    },
                ));
            }
            budget::preflight(&bytes)?;
            let decoded = serde_json::from_slice(&bytes)
                .map_err(|_| failure("invalid remote database response"))?;
            // A timeout cannot preempt synchronous JSON decoding. Its input and
            // workspace are bounded, and an over-deadline result is rejected.
            if started.elapsed() >= self.timeout {
                return Err(failure("remote database request timed out"));
            }
            Ok(decoded)
        })
        .await
        .map_err(|_| failure("remote database request timed out"))?
    }
}

impl fmt::Debug for RemoteConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteConnection")
            .field("credentials", &"<REDACTED>")
            .field("poisoned", &self.poisoned.get())
            .finish_non_exhaustive()
    }
}

struct BoundedWriter(Vec<u8>);
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > options::MAX_BODY.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("remote request limit"));
        }
        reserve_bounded(&mut self.0, bytes.len(), options::MAX_BODY)
            .map_err(|_| std::io::Error::other("remote request limit"))?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn reserve_bounded(bytes: &mut Vec<u8>, additional: usize, limit: usize) -> std::io::Result<()> {
    let required = bytes
        .len()
        .checked_add(additional)
        .filter(|length| *length <= limit)
        .ok_or_else(|| std::io::Error::other("remote buffer limit"))?;
    if required > bytes.capacity() {
        // Retain amortized growth without reserving beyond the configured cap.
        let capacity = required
            .checked_next_power_of_two()
            .unwrap_or(limit)
            .min(limit);
        bytes
            .try_reserve_exact(capacity - bytes.len())
            .map_err(|_| std::io::Error::other("remote buffer limit"))?;
    }
    Ok(())
}

fn execute_sql(sql: &str, want_rows: bool) -> StreamRequest {
    StreamRequest::Execute(ExecuteStreamReq {
        stmt: Stmt::new(sql, want_rows),
    })
}

fn matches_response(request: &StreamRequest, response: &StreamResponse) -> bool {
    matches!(
        (request, response),
        (StreamRequest::Execute(_), StreamResponse::Execute(_))
            | (StreamRequest::Describe(_), StreamResponse::Describe(_))
            | (StreamRequest::Sequence(_), StreamResponse::Sequence(_))
            | (StreamRequest::StoreSql(_), StreamResponse::StoreSql(_))
            | (StreamRequest::Close(_), StreamResponse::Close(_))
    )
}

fn expect_ok(result: StreamResult) -> Result<StreamResponse> {
    match result {
        StreamResult::Ok { response } => Ok(response),
        StreamResult::Error { error } => Err(sql_failure(&error.code)),
        StreamResult::None => Err(failure("invalid remote database response")),
    }
}

fn encode_value(value: Value) -> proto::Value {
    match value {
        Value::Null => proto::Value::Null,
        Value::Integer(value) => proto::Value::Integer { value },
        Value::Real(value) => proto::Value::Float { value },
        Value::Text(value) => proto::Value::Text {
            value: value.into(),
        },
        Value::Blob(value) => proto::Value::Blob {
            value: value.into(),
        },
    }
}

pub(crate) fn decode_value(value: proto::Value) -> Result<Value> {
    Ok(match value {
        proto::Value::Null => Value::Null,
        proto::Value::Integer { value } => Value::Integer(value),
        proto::Value::Float { value } if value.is_finite() => Value::Real(value),
        proto::Value::Text { value } => Value::Text(value.to_string()),
        proto::Value::Blob { value } => Value::Blob(value.to_vec()),
        _ => return Err(failure("invalid remote database response")),
    })
}

pub(crate) fn failure(message: &'static str) -> Error {
    Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
        Some(message.into()),
    )
}

fn sql_failure(code: &str) -> Error {
    use rusqlite::ffi;
    // Never retain the provider's message or code: both are untrusted and may
    // contain SQL, parameter values, endpoint details, or credentials.
    let code = match code {
        "SQLITE_CONSTRAINT_UNIQUE" => ffi::SQLITE_CONSTRAINT_UNIQUE,
        "SQLITE_CONSTRAINT_PRIMARYKEY" => ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
        "SQLITE_CONSTRAINT_FOREIGNKEY" => ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
        "SQLITE_CONSTRAINT_NOTNULL" => ffi::SQLITE_CONSTRAINT_NOTNULL,
        "SQLITE_CONSTRAINT_CHECK" => ffi::SQLITE_CONSTRAINT_CHECK,
        "SQLITE_CONSTRAINT" => ffi::SQLITE_CONSTRAINT,
        "SQLITE_BUSY" => ffi::SQLITE_BUSY,
        "SQLITE_LOCKED" => ffi::SQLITE_LOCKED,
        "SQLITE_READONLY" => ffi::SQLITE_READONLY,
        "SQLITE_FULL" => ffi::SQLITE_FULL,
        "SQLITE_TOOBIG" => ffi::SQLITE_TOOBIG,
        "SQLITE_CORRUPT" => ffi::SQLITE_CORRUPT,
        "SQLITE_NOTADB" => ffi::SQLITE_NOTADB,
        _ => ffi::SQLITE_ERROR,
    };
    Error::SqliteFailure(
        ffi::Error::new(code),
        Some("remote database statement failed".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::{RemoteConnection, RemoteOptions};

    #[test]
    fn same_origin_base_url_preserves_its_path_prefix() {
        let connection = RemoteConnection::connect(RemoteOptions::new(
            "https://synthetic.invalid",
            "synthetic-token",
        ))
        .unwrap();
        for base in [
            "https://synthetic.invalid/route",
            "https://synthetic.invalid/route/",
        ] {
            assert_eq!(
                connection
                    .transport
                    .next_pipeline(Some(base))
                    .unwrap()
                    .path(),
                "/route/v2/pipeline"
            );
        }
        assert_eq!(
            connection
                .transport
                .next_pipeline(Some("https://synthetic.invalid"))
                .unwrap()
                .path(),
            "/v2/pipeline"
        );
    }
}
