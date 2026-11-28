//! Keep-alive loopback proxy for the connection-reuse fault contracts.
//!
//! The proxy stands in for the database: a request counts as "received" when
//! its complete body has been read. It answers with `Connection: keep-alive`
//! so the adapter may reuse the socket, and can inject the three faults of
//! remote-performance requirement 6.3.
#![allow(dead_code)] // Only the keep-alive binary uses this fixture.
use super::client::{BODY_LIMIT, REQUEST_TIMEOUT, bounded_body, http_client};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Forward a request that arrives on a reused socket, then close the
    /// socket without answering (the database may have applied it).
    DropBeforeResponse,
    /// After each answer, close the idle socket after this delay.
    CloseIdleAfter(Duration),
    /// Read a request that arrives on a reused socket completely, then close
    /// without forwarding or answering.
    CloseAfterReceive,
}

/// One received request: socket number, position on that socket, body.
#[derive(Clone, Debug)]
pub struct Received {
    pub socket: usize,
    pub position: usize,
    pub body: Vec<u8>,
}

#[derive(Default)]
struct State {
    fault: Mutex<Option<Fault>>,
    sockets: AtomicUsize,
    received: Mutex<Vec<Received>>,
    forwarded: AtomicUsize,
    triggered: AtomicUsize,
}

pub struct KeepAliveProxy {
    address: SocketAddr,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl KeepAliveProxy {
    pub fn start(upstream: SocketAddr, fault: Option<Fault>) -> io::Result<Self> {
        if !upstream.ip().is_loopback() {
            return Err(io::Error::other("fixture upstream must be loopback"));
        }
        let runtime = meibo_db::runtime()?;
        let _entered = runtime.enter();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let state = Arc::new(State {
            fault: Mutex::new(fault),
            ..State::default()
        });
        let shared = state.clone();
        let client = http_client()?;
        let (shutdown, mut stopped) = oneshot::channel();
        let task = runtime.spawn(async move {
            let mut sockets = JoinSet::new();
            while matches!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                match tokio::time::timeout(Duration::from_millis(10), listener.accept()).await {
                    Ok(Ok((stream, _))) => {
                        let socket = shared.sockets.fetch_add(1, Ordering::SeqCst);
                        let state = shared.clone();
                        let client = client.clone();
                        sockets.spawn(async move {
                            let _ = serve(stream, socket, upstream, &state, &client).await;
                        });
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {}
                }
                while sockets.try_join_next().is_some() {}
            }
            sockets.shutdown().await;
        });
        Ok(Self {
            address,
            state,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn sockets(&self) -> usize {
        self.state.sockets.load(Ordering::SeqCst)
    }

    pub fn forwarded(&self) -> usize {
        self.state.forwarded.load(Ordering::SeqCst)
    }

    pub fn triggered(&self) -> usize {
        self.state.triggered.load(Ordering::SeqCst)
    }

    pub fn received(&self) -> Vec<Received> {
        self.state.received.lock().unwrap().clone()
    }
}

impl Drop for KeepAliveProxy {
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

async fn serve(
    mut stream: TcpStream,
    socket: usize,
    upstream: SocketAddr,
    state: &State,
    client: &reqwest::Client,
) -> io::Result<()> {
    for position in 0.. {
        let Some((path, authorization, body)) = read_request(&mut stream).await? else {
            return Ok(());
        };
        state.received.lock().unwrap().push(Received {
            socket,
            position,
            body: body.clone(),
        });
        let fault = *state.fault.lock().unwrap();
        let reused = position > 0;
        if fault == Some(Fault::CloseAfterReceive) && reused {
            state.triggered.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        let mut request = client
            .post(format!("http://{upstream}{path}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(authorization) = authorization {
            request = request.header(reqwest::header::AUTHORIZATION, authorization);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| io::Error::other("fixture upstream failed"))?;
        state.forwarded.fetch_add(1, Ordering::SeqCst);
        let status = response.status();
        let body = bounded_body(&mut response).await?;
        if fault == Some(Fault::DropBeforeResponse) && reused {
            state.triggered.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        stream
            .write_all(
                format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    status.as_str(),
                    status.canonical_reason().unwrap_or_default(),
                    body.len()
                )
                .as_bytes(),
            )
            .await?;
        stream.write_all(&body).await?;
        stream.flush().await?;
        if let Some(Fault::CloseIdleAfter(delay)) = fault {
            tokio::time::sleep(delay).await;
            state.triggered.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
    }
    Ok(())
}

/// Read one bounded request, or `None` when the client closed an idle socket.
async fn read_request(
    stream: &mut TcpStream,
) -> io::Result<Option<(String, Option<String>, Vec<u8>)>> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() == 8192 {
            return Err(io::Error::other("fixture request headers exceed limit"));
        }
        let mut byte = [0];
        if stream.read(&mut byte).await? == 0 {
            return if header.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::other("fixture request truncated"))
            };
        }
        header.push(byte[0]);
    }
    let text = std::str::from_utf8(&header)
        .map_err(|_| io::Error::other("invalid fixture request headers"))?;
    let mut lines = text.split("\r\n");
    let path = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| io::Error::other("invalid fixture request line"))?
        .to_owned();
    let mut length = 0;
    let mut authorization = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value
                    .trim()
                    .parse()
                    .map_err(|_| io::Error::other("invalid fixture content length"))?;
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_owned());
            }
        }
    }
    if length > BODY_LIMIT {
        return Err(io::Error::other("fixture request body exceeds limit"));
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(Some((path, authorization, body)))
}
