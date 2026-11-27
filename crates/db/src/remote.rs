//! Bounded Hrana v2 transport and complete non-transactional SQL operations.
mod budget;
mod diagnostics;
mod error_kind;
mod options;
mod sql;
mod state;

pub use diagnostics::{RemoteTransportDiagnostic, RemoteTransportKind, RemoteTransportPhase};
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
    collections::HashMap,
    fmt,
    io::Write,
    sync::Mutex,
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
    described: RefCell<HashMap<String, Described>>,
}

/// One step of [`RemoteConnection::read_batch`].
#[derive(Clone, Debug)]
pub enum ReadStep<'sql> {
    /// Return every row of one read-only SELECT.
    Query(&'sql str, Vec<Value>),
    /// Confirm that the SQL still compiles against the current schema.
    Check(&'sql str),
}

/// Successful Describe facts. They are scoped to this connection and cleared
/// before any statement that may change the schema.
#[derive(Clone, Copy)]
struct Described {
    parameters: usize,
    columns: usize,
    read_only: bool,
}

/// Bound on remembered statements; the application uses far fewer.
const DESCRIBED_LIMIT: usize = 256;

/// Whether read-only round trips inside a [`ReadFence`] may reuse an idle
/// HTTPS connection. Outside a fence every round trip uses a new one.
///
/// Requirement 6 of remote-performance: enable only after the keep-alive
/// fault tests prove that the database receives each round trip at most once.
/// Writes, transaction stages and gate changes always use a new connection.
pub const REUSE_READ_CONNECTIONS: bool = true;
/// Idle lifetime of the single reusable read connection (requirement 6.5).
const REUSE_IDLE: std::time::Duration = std::time::Duration::from_secs(4);

thread_local! {
    static FENCE: FenceState = const { FenceState::new() };
    static USAGE: Cell<RemoteUsage> = const { Cell::new(RemoteUsage::ZERO) };
}

/// Round trips sent from this thread and the row counts the provider reported
/// for them (`rows_read`/`rows_written` of each statement result).
///
/// Counted per thread rather than per connection so that a connection opened
/// and dropped inside one request is still included. A provider that omits the
/// row counts contributes zero rows; the round trip is still counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteUsage {
    /// Pipeline requests sent, including failed ones.
    pub round_trips: u64,
    /// Rows the provider reported as read.
    pub rows_read: u64,
    /// Rows the provider reported as written.
    pub rows_written: u64,
}

impl RemoteUsage {
    /// No usage.
    pub const ZERO: Self = Self {
        round_trips: 0,
        rows_read: 0,
        rows_written: 0,
    };

    /// Return this thread's usage since the last call and reset it to zero.
    #[must_use]
    pub fn take() -> Self {
        USAGE.with(|usage| usage.replace(Self::ZERO))
    }

    fn add(response: Option<&PipelineRespBody>) {
        let (mut read, mut written) = (0_u64, 0_u64);
        for result in response.map_or(&[][..], |response| response.results.as_slice()) {
            if let StreamResult::Ok {
                response: StreamResponse::Execute(execute),
            } = result
            {
                read = read.saturating_add(execute.result.rows_read);
                written = written.saturating_add(execute.result.rows_written);
            }
        }
        USAGE.with(|usage| {
            let current = usage.get();
            usage.set(Self {
                round_trips: current.round_trips.saturating_add(1),
                rows_read: current.rows_read.saturating_add(read),
                rows_written: current.rows_written.saturating_add(written),
            });
        });
    }
}

struct FenceState {
    depth: Cell<usize>,
    tripped: Cell<bool>,
    failed: Cell<bool>,
}

impl FenceState {
    const fn new() -> Self {
        Self {
            depth: Cell::new(0),
            tripped: Cell::new(false),
            failed: Cell::new(false),
        }
    }
}

/// While held, every remote connection used on this thread, including one
/// opened after the fence began, refuses a statement that may write before
/// anything is sent.
///
/// A refusal never poisons the connection and never uses an uncertain-outcome
/// classification. [`ReadFence::tripped`] reports whether anything was
/// refused. Before retrying a refusal on the serialized path, the caller
/// must check [`ReadFence::failed`], including failures during cleanup.
/// Dropping the last fence restores writes and clears both flags.
#[must_use = "the fence is active only while this value is held"]
pub struct ReadFence {
    // Thread-bound: the fence must end on the thread that began it.
    _thread: std::marker::PhantomData<*const ()>,
}

impl ReadFence {
    fn active() -> bool {
        FENCE.with(|fence| fence.depth.get() > 0)
    }

    /// Begin refusing possible writes on this thread.
    pub fn begin() -> Self {
        FENCE.with(|fence| fence.depth.set(fence.depth.get() + 1));
        Self {
            _thread: std::marker::PhantomData,
        }
    }

    /// Whether a possible write was refused since the outermost fence began.
    #[must_use]
    pub fn tripped(&self) -> bool {
        FENCE.with(|fence| fence.tripped.get())
    }

    /// Whether a transport or protocol failure invalidated a connection since
    /// the outermost fence began. The flag survives dropping that connection.
    #[must_use]
    pub fn failed(&self) -> bool {
        FENCE.with(|fence| fence.failed.get())
    }
}

impl Drop for ReadFence {
    fn drop(&mut self) {
        FENCE.with(|fence| {
            let depth = fence.depth.get().saturating_sub(1);
            fence.depth.set(depth);
            if depth == 0 {
                fence.tripped.set(false);
                fence.failed.set(false);
            }
        });
    }
}

impl fmt::Debug for ReadFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadFence")
            .field("tripped", &self.tripped())
            .field("failed", &self.failed())
            .finish()
    }
}

struct Transport {
    http: reqwest::Client,
    reuse: Option<reqwest::Client>,
    pipeline: Url,
    authorization: HeaderValue,
    timeout: std::time::Duration,
    response_limit: usize,
    first_failure: Mutex<Option<RemoteTransportDiagnostic>>,
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
        // Hyper can retry a canceled checkout on a reused HTTP/1 socket
        // independently of reqwest's policy. Data-changing round trips never
        // use an idle socket; see `REUSE_READ_CONNECTIONS` for reads.
        let http = client(timeout, 0, None)?;
        let reuse = if REUSE_READ_CONNECTIONS {
            Some(client(timeout, 1, Some(REUSE_IDLE))?)
        } else {
            None
        };
        Ok(Self {
            transport: Transport {
                http,
                reuse,
                pipeline,
                authorization,
                timeout,
                response_limit,
                first_failure: Mutex::new(None),
            },
            poisoned: Cell::new(false),
            last_insert_rowid: Cell::new(0),
            active: RefCell::new(None),
            described: RefCell::new(HashMap::new()),
        })
    }

    /// Reject a possible write inside a fence before anything is sent.
    #[allow(clippy::unused_self)] // Scoped with the connection's other guards.
    fn check_fence(&self, may_write: bool) -> Result<()> {
        if may_write && ReadFence::active() {
            FENCE.with(|fence| fence.tripped.set(true));
            return Err(RemoteErrorKind::Unsupported.error());
        }
        Ok(())
    }

    fn inspect_sql(&self, sql: &str, in_transaction: bool) -> Result<sql::Effects> {
        let effects = sql::inspect(sql, in_transaction);
        if effects.is_err() {
            // Locally rejected transaction-control SQL also trips a read fence.
            self.check_fence(true)?;
        }
        effects
    }

    /// Run read-only steps in one closing round trip and return each result.
    ///
    /// Every [`ReadStep::Query`] must be one SELECT that the provider has
    /// described as read-only on this connection (an uncached statement costs
    /// one extra Describe round trip first). [`ReadStep::Check`] always sends a
    /// fresh Describe and fails unless the SQL still compiles. The steps share
    /// one deferred read transaction, so they observe one snapshot.
    ///
    /// # Errors
    /// Returns the first failed step, a sanitized transport error, or
    /// `Unsupported` inside a transaction or for a step that may write.
    pub fn read_batch(&self, steps: &[ReadStep<'_>]) -> Result<Vec<Vec<Vec<Value>>>> {
        self.ensure_usable()?;
        if self.active.borrow().is_some() {
            return Err(RemoteErrorKind::Unsupported.error());
        }
        // Validate the whole batch before the first uncached Query sends a Describe.
        for step in steps {
            let (ReadStep::Query(sql, _) | ReadStep::Check(sql)) = step;
            let effects = self.inspect_sql(sql, false)?;
            self.check_fence(!effects.single_select)?;
            if matches!(step, ReadStep::Query(..)) && !effects.single_select {
                return Err(RemoteErrorKind::Unsupported.error());
            }
        }
        let mut requests = vec![execute_sql("BEGIN DEFERRED", false)];
        for step in steps {
            match step {
                ReadStep::Query(sql, values) => {
                    let described = self.describe(sql)?;
                    if !described.read_only {
                        self.check_fence(true)?;
                        return Err(RemoteErrorKind::Unsupported.error());
                    }
                    crate::statement::check_parameters(values.len(), described.parameters)?;
                    let mut stmt = Stmt::new(*sql, true);
                    stmt.args = values.iter().cloned().map(encode_value).collect();
                    requests.push(StreamRequest::Execute(ExecuteStreamReq { stmt }));
                }
                ReadStep::Check(sql) => {
                    requests.push(StreamRequest::Describe(DescribeStreamReq {
                        sql: Some((*sql).to_owned()),
                        ..DescribeStreamReq::default()
                    }));
                }
            }
        }
        requests.push(execute_sql("COMMIT", false));
        requests.push(StreamRequest::Close(CloseStreamReq {}));
        let response = self.pipeline_with(&self.transport.pipeline, None, requests, true, true)?;
        let mut results = response.results.into_iter();
        // The pipeline already checked one result per request.
        let mut next = || results.next().unwrap_or(StreamResult::None);
        let begun = expect_ok(next());
        let mut outputs = Vec::with_capacity(steps.len());
        let mut first_error = None;
        for step in steps {
            let result = expect_ok(next());
            match (step, result) {
                (ReadStep::Query(..), Ok(StreamResponse::Execute(response))) => {
                    let width = response.result.cols.len();
                    let mut rows = Vec::with_capacity(response.result.rows.len());
                    for row in response.result.rows {
                        if row.values.len() != width {
                            return self.invalid_response();
                        }
                        rows.push(
                            row.values
                                .into_iter()
                                .map(decode_value)
                                .collect::<Result<Vec<_>>>()
                                .or_else(|_| self.invalid_response())?,
                        );
                    }
                    outputs.push(rows);
                }
                (ReadStep::Check(_), Ok(StreamResponse::Describe(_))) => outputs.push(Vec::new()),
                (_, Ok(_)) => return self.invalid_response(),
                (_, Err(error)) => {
                    first_error.get_or_insert(error);
                    outputs.push(Vec::new());
                }
            }
        }
        let committed = expect_ok(next());
        if expect_ok(next()).is_err() {
            return self.invalid_response();
        }
        begun?;
        if let Some(error) = first_error {
            return Err(error);
        }
        committed?;
        Ok(outputs)
    }

    fn invalidate_described(&self, schema_change: bool) {
        self.described.borrow_mut().clear();
        if schema_change && let Some(stream) = self.active.borrow_mut().as_mut() {
            stream.schema_changed = true;
        }
    }

    fn poison(&self) {
        self.poisoned.set(true);
        FENCE.with(|fence| {
            if fence.depth.get() > 0 {
                fence.failed.set(true);
            }
        });
    }

    /// Whether a transport or protocol failure has invalidated this connection.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.get()
    }

    /// Read fixed first-failure metadata for explicit operator diagnosis.
    ///
    /// It survives poisoning and never authorizes retrying an operation.
    #[must_use]
    pub fn last_transport_diagnostic(&self) -> Option<RemoteTransportDiagnostic> {
        *self
            .transport
            .first_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let described = self.describe(sql)?;
        Ok(Statement::from_remote(
            self,
            sql.to_owned(),
            described.parameters,
            described.columns,
            described.read_only,
        ))
    }

    fn describe(&self, sql: &str) -> Result<Described> {
        self.ensure_usable()?;
        let in_transaction = self.active.borrow().is_some();
        let effects = self.inspect_sql(sql, in_transaction)?;
        if let Some(described) = self.described.borrow().get(sql) {
            return Ok(*described);
        }
        self.check_fence(!effects.single_select)?;
        let request = StreamRequest::Describe(DescribeStreamReq {
            sql: Some(sql.to_owned()),
            ..DescribeStreamReq::default()
        });
        // Describe never executes SQL, so outside a transaction it needs no
        // foreign-key setup and uses one closing round trip.
        let response = if in_transaction {
            self.request(request, true)?
        } else {
            self.request_read(request)?
        };
        let StreamResponse::Describe(response) = response else {
            return self.invalid_response();
        };
        let described = Described {
            parameters: response.result.params.len(),
            columns: response.result.cols.len(),
            read_only: response.result.is_readonly,
        };
        // This schema may disappear on rollback (including to a savepoint).
        if self
            .active
            .borrow()
            .as_ref()
            .is_some_and(|stream| stream.schema_changed)
        {
            return Ok(described);
        }
        let mut cache = self.described.borrow_mut();
        if cache.len() >= DESCRIBED_LIMIT {
            cache.clear();
        }
        cache.insert(sql.to_owned(), described);
        Ok(described)
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
        self.check_fence(true)?;
        let effects = sql::inspect(sql, self.active.borrow().is_some())?;
        self.invalidate_described(effects.schema);
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
        let in_transaction = self.active.borrow().is_some();
        let effects = self.inspect_sql(sql, in_transaction)?;
        let read_select = read_only && effects.single_select;
        self.check_fence(!read_select)?;
        if effects.schema {
            self.invalidate_described(true);
        }
        let mut stmt = Stmt::new(sql, want_rows);
        stmt.args = values.into_iter().map(encode_value).collect();
        let request = StreamRequest::Execute(ExecuteStreamReq { stmt });
        // Both the provider and the local inspection must agree that this is
        // one read-only SELECT before it skips the write-side FK setup.
        let response = if read_select && !in_transaction {
            self.request_read(request)?
        } else {
            self.request(request, read_only)?
        };
        match response {
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
        self.transport.record_failure(
            RemoteTransportPhase::ResponseDecode,
            RemoteTransportKind::Invalid,
        );
        self.poison();
        Err(RemoteErrorKind::OutcomeUnknown.error())
    }

    fn pipeline(
        &self,
        url: &Url,
        baton: Option<String>,
        requests: Vec<StreamRequest>,
        closing: bool,
    ) -> Result<PipelineRespBody> {
        self.pipeline_with(url, baton, requests, closing, false)
    }

    /// `reuse` is true only for a closing round trip that cannot change data.
    fn pipeline_with(
        &self,
        url: &Url,
        baton: Option<String>,
        requests: Vec<StreamRequest>,
        closing: bool,
        reuse: bool,
    ) -> Result<PipelineRespBody> {
        self.ensure_usable()?;
        // Requirement 6.1/6.2: only a gate-free read (inside a fence on this
        // thread) may reuse a socket. Everything else uses a new connection.
        let reuse = reuse && ReadFence::active();
        let body = PipelineReqBody { baton, requests };
        let result = self.transport.pipeline(url, &body, closing, reuse);
        RemoteUsage::add(result.as_ref().ok());
        if result.is_err() {
            self.poison();
        }
        result
    }
}

impl Transport {
    fn record_failure(&self, phase: RemoteTransportPhase, kind: RemoteTransportKind) {
        self.first_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(RemoteTransportDiagnostic { phase, kind });
    }

    fn failed(
        &self,
        phase: RemoteTransportPhase,
        kind: RemoteTransportKind,
        message: &'static str,
    ) -> Error {
        self.record_failure(phase, kind);
        failure(message)
    }

    fn invalid_response(&self) -> Error {
        self.failed(
            RemoteTransportPhase::ResponseDecode,
            RemoteTransportKind::Invalid,
            "invalid remote database response",
        )
    }

    fn response_limit(&self) -> Error {
        self.failed(
            RemoteTransportPhase::ResponseBody,
            RemoteTransportKind::Limit,
            "remote database response exceeds limit",
        )
    }

    fn check_status(&self, status: reqwest::StatusCode) -> Result<()> {
        if status.is_success() {
            return Ok(());
        }
        let (kind, message) = if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            (
                RemoteTransportKind::Authentication,
                "remote database authentication failed",
            )
        } else {
            (
                RemoteTransportKind::Status,
                "remote database request rejected",
            )
        };
        Err(self.failed(RemoteTransportPhase::ResponseBody, kind, message))
    }
    fn pipeline(
        &self,
        url: &Url,
        body: &PipelineReqBody,
        closing: bool,
        reuse: bool,
    ) -> Result<PipelineRespBody> {
        let http = match (&self.reuse, reuse) {
            (Some(client), true) => client,
            _ => &self.http,
        };
        let mut encoded = BoundedWriter(Vec::new());
        serde_json::to_writer(&mut encoded, body)
            .map_err(|_| failure("remote database request exceeds limit"))?;
        let runtime =
            crate::runtime().map_err(|_| failure("remote database runtime unavailable"))?;
        let run = || runtime.block_on(self.receive(http, url, encoded.0));
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
            return Err(self.invalid_response());
        }
        // Validate the server origin before any token-bearing follow-up.
        self.next_pipeline(response.base_url.as_deref())?;
        for (request, result) in body.requests.iter().zip(&response.results) {
            match result {
                StreamResult::Ok { response } if matches_response(request, response) => {}
                StreamResult::Error { .. } => {}
                _ => {
                    return Err(self.invalid_response());
                }
            }
        }
        Ok(response)
    }

    fn next_pipeline(&self, base_url: Option<&str>) -> Result<Url> {
        let Some(base) = base_url else {
            return Ok(self.pipeline.clone());
        };
        if !options::safe_raw_url(base) {
            return Err(self.invalid_response());
        }
        let mut base = Url::parse(base).map_err(|_| self.invalid_response())?;
        if !options::safe_url(&base)
            || !matches!(base.scheme(), "http" | "https")
            || base.origin() != self.pipeline.origin()
        {
            return Err(self.invalid_response());
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        base.join("v2/pipeline")
            .map_err(|_| self.invalid_response())
    }

    async fn receive(
        &self,
        http: &reqwest::Client,
        url: &Url,
        body: Vec<u8>,
    ) -> Result<PipelineRespBody> {
        let started = std::time::Instant::now();
        let phase = Cell::new(RemoteTransportPhase::Request);
        // The outer deadline also covers processing between received chunks.
        tokio::time::timeout(self.timeout, async {
            let mut response = http
                .post(url.clone())
                .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|error| {
                    self.failed(
                        phase.get(),
                        diagnostics::classify(&error),
                        "remote database request failed",
                    )
                })?;
            phase.set(RemoteTransportPhase::ResponseBody);
            let status = response.status();
            if response
                .content_length()
                .is_some_and(|length| length > self.response_limit as u64)
            {
                return Err(self.response_limit());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                self.failed(
                    phase.get(),
                    diagnostics::classify(&error),
                    "remote database response interrupted",
                )
            })? {
                if chunk.len() > self.response_limit.saturating_sub(bytes.len()) {
                    return Err(self.response_limit());
                }
                reserve_bounded(&mut bytes, chunk.len(), self.response_limit)
                    .map_err(|_| self.response_limit())?;
                bytes.extend_from_slice(&chunk);
            }
            self.check_status(status)?;
            phase.set(RemoteTransportPhase::ResponseDecode);
            budget::preflight(&bytes)
                .inspect_err(|_| self.record_failure(phase.get(), RemoteTransportKind::Invalid))?;
            let decoded = serde_json::from_slice(&bytes).map_err(|_| self.invalid_response())?;
            // A timeout cannot preempt synchronous JSON decoding. Its input and
            // workspace are bounded, and an over-deadline result is rejected.
            if started.elapsed() >= self.timeout {
                return Err(self.failed(
                    phase.get(),
                    RemoteTransportKind::Timeout,
                    "remote database request timed out",
                ));
            }
            Ok(decoded)
        })
        .await
        .map_err(|_| {
            self.failed(
                phase.get(),
                RemoteTransportKind::Timeout,
                "remote database request timed out",
            )
        })?
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

/// Both clients share every TLS, redirect and retry restriction. Only the
/// idle pool differs; `idle == 0` keeps the original fresh-connection client.
fn client(
    timeout: std::time::Duration,
    idle: usize,
    idle_timeout: Option<std::time::Duration>,
) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(idle);
    if let Some(idle_timeout) = idle_timeout {
        builder = builder.pool_idle_timeout(idle_timeout);
    }
    builder
        .timeout(timeout)
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .tls_danger_accept_invalid_hostnames(false)
        .tls_danger_accept_invalid_certs(false)
        .connection_verbose(false)
        .build()
        .map_err(|_| failure("remote database client unavailable"))
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
    use super::{ReadFence, RemoteConnection, RemoteOptions};

    fn connection() -> RemoteConnection {
        RemoteConnection::connect(RemoteOptions::new(
            "https://synthetic.invalid",
            "synthetic-token",
        ))
        .unwrap()
    }

    #[test]
    fn failure_lasts_until_outer_fence_ends() {
        let outer = ReadFence::begin();
        {
            let inner = ReadFence::begin();
            assert!(!inner.failed());
            // The failing connection is dropped before the fence is inspected.
            assert!(connection().invalid_response::<()>().is_err());
            assert!(inner.failed());
            assert!(!inner.tripped());
        }
        assert!(outer.failed());
        drop(outer);
        assert!(!ReadFence::begin().failed());
    }

    #[test]
    fn same_origin_base_url_preserves_its_path_prefix() {
        let connection = connection();
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
