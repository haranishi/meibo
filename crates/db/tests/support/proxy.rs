use super::client::{BODY_LIMIT, REQUEST_TIMEOUT, bounded_body, http_client};
use libsql_hrana::proto::{
    PipelineReqBody, PipelineRespBody, StreamRequest, StreamResponse, StreamResult,
};
use reqwest::{
    Method,
    header::{CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue, TRANSFER_ENCODING},
};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

const HEADER_LIMIT: usize = 8192;

#[derive(Clone, Copy)]
pub enum Fault {
    DelayResponse(Duration),
    OversizedErrorBody(usize),
    LoseCommitResponse,
    #[allow(dead_code)] // Used by the transaction binary, not foundation.rs.
    LoseRollbackResponse,
    #[allow(dead_code)]
    LoseWriteResponse,
    #[allow(dead_code)] // Selected by application-level integration contracts.
    LoseMatchingWriteResponse(&'static str),
    #[allow(dead_code)]
    RoutePrefix,
}

#[derive(Default)]
struct State {
    fault: Mutex<Option<Fault>>,
    triggered: AtomicUsize,
    requests: AtomicUsize,
    commits: AtomicUsize,
    rollbacks: AtomicUsize,
    routed: AtomicBool,
}

pub struct FaultProxy {
    address: SocketAddr,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl FaultProxy {
    pub fn start(upstream: SocketAddr) -> io::Result<Self> {
        if !upstream.ip().is_loopback() {
            return Err(io::Error::other("fixture upstream must be loopback"));
        }
        let runtime = meibo_db::runtime()?;
        let _entered = runtime.enter();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let state = Arc::new(State::default());
        let shared = state.clone();
        let client = http_client()?;
        let (shutdown, mut stopped) = oneshot::channel();
        let task = runtime.spawn(async move {
            let mut requests = JoinSet::new();
            while matches!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                match tokio::time::timeout(Duration::from_millis(10), listener.accept()).await {
                    Ok(Ok((stream, _))) => {
                        let state = shared.clone();
                        let client = client.clone();
                        requests.spawn(async move {
                            let _ = tokio::time::timeout(
                                REQUEST_TIMEOUT,
                                forward(stream, upstream, &state, &client),
                            )
                            .await;
                        });
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {}
                }
                while requests.try_join_next().is_some() {}
            }
            requests.shutdown().await;
        });
        Ok(Self {
            address,
            state,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn arm(&self, fault: Fault) {
        let mut armed = self.state.fault.lock().unwrap();
        assert!(armed.is_none(), "a fixture fault is already armed");
        *armed = Some(fault);
    }

    pub fn triggered(&self) -> usize {
        self.state.triggered.load(Ordering::SeqCst)
    }

    #[allow(dead_code)] // Shared fixture methods are used by different test binaries.
    pub fn requests(&self) -> usize {
        self.state.requests.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn commits(&self) -> usize {
        self.state.commits.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn rollbacks(&self) -> usize {
        self.state.rollbacks.load(Ordering::SeqCst)
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let (Some(mut task), Ok(runtime)) = (self.task.take(), meibo_db::runtime()) {
            runtime.block_on(async {
                if tokio::time::timeout(REQUEST_TIMEOUT, &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            });
        }
    }
}

async fn forward(
    mut stream: TcpStream,
    upstream: SocketAddr,
    state: &State,
    client: &reqwest::Client,
) -> io::Result<()> {
    let mut request = read_request(&mut stream).await?;
    state.requests.fetch_add(1, Ordering::SeqCst);
    let commit = is_control(&request.body, "COMMIT");
    let rollback = is_control(&request.body, "ROLLBACK");
    let write = is_insert(&request.body);
    if commit {
        state.commits.fetch_add(1, Ordering::SeqCst);
    }
    if rollback {
        state.rollbacks.fetch_add(1, Ordering::SeqCst);
    }
    let continuing = serde_json::from_slice::<PipelineReqBody>(&request.body)
        .is_ok_and(|body| body.baton.is_some());
    if state.routed.load(Ordering::SeqCst) && continuing {
        let Some(path) = request.path.strip_prefix("/route") else {
            return Err(io::Error::other("fixture stream lost its route prefix"));
        };
        request.path = path.to_owned();
    }
    let fault = {
        let mut armed = state.fault.lock().unwrap();
        match *armed {
            Some(Fault::LoseCommitResponse) if !commit => None,
            Some(Fault::LoseRollbackResponse) if !rollback => None,
            Some(Fault::LoseWriteResponse) if !write => None,
            Some(Fault::LoseMatchingWriteResponse(prefix))
                if !matching_execute(&request.body, prefix) =>
            {
                None
            }
            _ => armed.take(),
        }
    };
    if let Some(Fault::OversizedErrorBody(length)) = fault {
        state.triggered.fetch_add(1, Ordering::SeqCst);
        return oversized_error(&mut stream, length).await;
    }
    let mut response = client
        .request(request.method, format!("http://{upstream}{}", request.path))
        .headers(request.headers)
        .body(request.body)
        .send()
        .await
        .map_err(|_| io::Error::other("fixture upstream failed"))?;
    let status = response.status();
    let mut body = bounded_body(&mut response).await?;
    match fault {
        Some(Fault::DelayResponse(duration)) => {
            state.triggered.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(duration).await;
        }
        Some(Fault::LoseCommitResponse | Fault::LoseRollbackResponse) => {
            if !status.is_success() || !commit_accepted(&body) {
                return Err(io::Error::other("fixture commit was rejected"));
            }
            state.triggered.fetch_add(1, Ordering::SeqCst);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "synthetic commit response loss",
            ));
        }
        Some(Fault::LoseWriteResponse | Fault::LoseMatchingWriteResponse(_)) => {
            let accepted = lost_write_accepted(fault, &body);
            if !status.is_success() || !accepted {
                return Err(io::Error::other("fixture write was rejected"));
            }
            state.triggered.fetch_add(1, Ordering::SeqCst);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "synthetic write response loss",
            ));
        }
        Some(Fault::RoutePrefix) => {
            let mut decoded: PipelineRespBody = serde_json::from_slice(&body)
                .map_err(|_| io::Error::other("invalid fixture routing response"))?;
            decoded.base_url = Some(format!("http://{}/route/", stream.local_addr()?));
            body = serde_json::to_vec(&decoded)
                .map_err(|_| io::Error::other("invalid fixture routing response"))?;
            state.routed.store(true, Ordering::SeqCst);
            state.triggered.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
    stream
        .write_all(
            format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status.as_str(),
                status.canonical_reason().unwrap_or_default(),
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(&body).await?;
    stream.shutdown().await
}

struct Request {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Request> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() == HEADER_LIMIT {
            return Err(io::Error::other("fixture request headers exceed limit"));
        }
        header.push(stream.read_u8().await?);
    }
    let header = std::str::from_utf8(&header)
        .map_err(|_| io::Error::other("invalid fixture request headers"))?;
    let mut lines = header.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = Method::from_bytes(request_line.next().unwrap_or_default().as_bytes())
        .map_err(|_| io::Error::other("invalid fixture request method"))?;
    let path = request_line.next().unwrap_or_default();
    if !path.starts_with('/') || request_line.next() != Some("HTTP/1.1") {
        return Err(io::Error::other("invalid fixture request path"));
    }
    let mut headers = HeaderMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("invalid fixture request header"))?;
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| io::Error::other("invalid fixture request header name"))?;
        let value = HeaderValue::from_str(value.trim())
            .map_err(|_| io::Error::other("invalid fixture request header value"))?;
        if headers.insert(name, value).is_some() {
            return Err(io::Error::other("duplicate fixture request header"));
        }
    }
    if headers.contains_key(TRANSFER_ENCODING) {
        return Err(io::Error::other(
            "fixture requires a bounded content length",
        ));
    }
    let length = headers.get(CONTENT_LENGTH).map_or(Ok(0), |value| {
        value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| io::Error::other("invalid fixture content length"))
    })?;
    if length > BODY_LIMIT {
        return Err(io::Error::other("fixture request body exceeds limit"));
    }
    for name in ["host", "connection", "content-length", "accept-encoding"] {
        headers.remove(name);
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(Request {
        method,
        path: path.to_owned(),
        headers,
        body,
    })
}

fn is_control(bytes: &[u8], control: &str) -> bool {
    serde_json::from_slice::<PipelineReqBody>(bytes).is_ok_and(|body| {
        body.requests.iter().any(|request| {
            matches!(request, StreamRequest::Execute(request) if request.stmt.sql.as_deref().is_some_and(|sql| {
                sql.trim().trim_end_matches(';').trim().eq_ignore_ascii_case(control)
            }))
        })
    })
}

fn commit_accepted(bytes: &[u8]) -> bool {
    serde_json::from_slice::<PipelineRespBody>(bytes).is_ok_and(|body| {
        matches!(
            body.results.as_slice(),
            [StreamResult::Ok {
                response: StreamResponse::Execute(_)
            }]
        )
    })
}

fn is_insert(bytes: &[u8]) -> bool {
    serde_json::from_slice::<PipelineReqBody>(bytes).is_ok_and(|body| {
        body.requests.iter().any(|request| {
            let sql = match request {
                StreamRequest::Execute(request) => request.stmt.sql.as_deref(),
                StreamRequest::Sequence(request) => request.sql.as_deref(),
                _ => None,
            };
            sql.is_some_and(|sql| sql.trim_start().starts_with("INSERT INTO synthetic"))
        })
    })
}

fn matching_execute(bytes: &[u8], prefix: &str) -> bool {
    serde_json::from_slice::<PipelineReqBody>(bytes).is_ok_and(|body| {
        body.requests.iter().any(|request| {
            matches!(request, StreamRequest::Execute(request)
                if request.stmt.sql.as_deref().is_some_and(|sql| sql.trim_start().starts_with(prefix)))
        })
    })
}

fn execute_accepted(bytes: &[u8]) -> bool {
    serde_json::from_slice::<PipelineRespBody>(bytes).is_ok_and(|body| {
        matches!(
            body.results.first(),
            Some(StreamResult::Ok {
                response: StreamResponse::Execute(_)
            })
        ) && body
            .results
            .iter()
            .all(|result| matches!(result, StreamResult::Ok { .. }))
    })
}

fn lost_write_accepted(fault: Option<Fault>, bytes: &[u8]) -> bool {
    match fault {
        Some(Fault::LoseWriteResponse) => write_accepted(bytes),
        Some(Fault::LoseMatchingWriteResponse(_)) => execute_accepted(bytes),
        _ => false,
    }
}

fn write_accepted(bytes: &[u8]) -> bool {
    serde_json::from_slice::<PipelineRespBody>(bytes).is_ok_and(|body| {
        matches!(
            body.results.first(),
            Some(StreamResult::Ok {
                response: StreamResponse::Execute(_) | StreamResponse::Sequence(_)
            })
        ) && matches!(
            body.results.last(),
            Some(StreamResult::Ok {
                response: StreamResponse::Close(_)
            })
        )
    })
}

async fn oversized_error(stream: &mut TcpStream, length: usize) -> io::Result<()> {
    stream
        .write_all(
            format!(
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let chunk = [b'x'; 8192];
    let mut remaining = length;
    while remaining > 0 {
        let count = remaining.min(chunk.len());
        stream.write_all(&chunk[..count]).await?;
        remaining -= count;
    }
    stream.shutdown().await
}
