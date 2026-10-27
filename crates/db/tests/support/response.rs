//! Raw loopback responses for adversarial wire tests (never the success fixture).
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct ResponseServer {
    address: SocketAddr,
    count: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
    requests: Arc<Mutex<Vec<libsql_hrana::proto::PipelineReqBody>>>,
}

impl ResponseServer {
    pub fn start(bytes: Vec<u8>, delay: Duration) -> Self {
        Self::script(vec![bytes], delay, false)
    }

    pub fn script(replies: Vec<Vec<u8>>, delay: Duration, delay_body_only: bool) -> Self {
        Self::script_inner(replies, delay, delay_body_only, false)
    }

    pub fn keepalive(replies: Vec<Vec<u8>>) -> Self {
        let replies = replies
            .into_iter()
            .map(|reply| {
                String::from_utf8(reply)
                    .unwrap()
                    .replacen("Connection: close\r\n", "Connection: keep-alive\r\n", 1)
                    .into_bytes()
            })
            .collect();
        Self::script_inner(replies, Duration::ZERO, false, true)
    }

    fn script_inner(
        replies: Vec<Vec<u8>>,
        delay: Duration,
        delay_body_only: bool,
        keepalive: bool,
    ) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let recorded = captured.clone();
        let requests = count.clone();
        let stop = stopped.clone();
        let task = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // BSD accepted sockets can inherit nonblocking mode.
                        // This fixture uses blocking IO with per-socket deadlines.
                        stream.set_nonblocking(false).unwrap();
                        requests.fetch_add(1, Ordering::SeqCst);
                        while let Ok(body) = read_request(&mut stream) {
                            let Ok(body) = serde_json::from_slice(&body) else {
                                break;
                            };
                            let index = {
                                let mut captured = recorded.lock().unwrap();
                                captured.push(body);
                                captured.len() - 1
                            };
                            let bytes = &replies[index.min(replies.len() - 1)];
                            let mut bytes = bytes.as_slice();
                            if delay_body_only {
                                let end = bytes
                                    .windows(4)
                                    .position(|chunk| chunk == b"\r\n\r\n")
                                    .unwrap()
                                    + 4;
                                let _ = stream.write_all(&bytes[..end]);
                                bytes = &bytes[end..];
                            }
                            thread::sleep(delay);
                            // Write incrementally to exercise receiving chunk bounds.
                            for chunk in bytes.chunks(257) {
                                if stream.write_all(chunk).is_err() {
                                    break;
                                }
                            }
                            if !keepalive {
                                break;
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            count,
            stopped,
            task: Some(task),
            requests: captured,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    pub fn request_types(&self) -> Vec<Vec<&'static str>> {
        use libsql_hrana::proto::StreamRequest;
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|body| {
                body.requests
                    .iter()
                    .map(|request| match request {
                        StreamRequest::Execute(_) => "execute",
                        StreamRequest::Describe(_) => "describe",
                        StreamRequest::Sequence(_) => "sequence",
                        StreamRequest::StoreSql(_) => "store_sql",
                        StreamRequest::Close(_) => "close",
                        _ => "other",
                    })
                    .collect()
            })
            .collect()
    }
}

impl Drop for ResponseServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        header.push(byte[0]);
        if header.len() > 8192 {
            return Err(std::io::Error::other("synthetic request header too long"));
        }
    }
    let text = String::from_utf8_lossy(&header);
    let size = text
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    if size > 1024 * 1024 {
        return Err(std::io::Error::other("synthetic request too long"));
    }
    let mut body = vec![0; size];
    stream.read_exact(&mut body)?;
    Ok(body)
}

pub fn response(status: u16, body: &[u8], chunked: bool, extra: &str) -> Vec<u8> {
    let framing = if chunked {
        "Transfer-Encoding: chunked\r\n".to_owned()
    } else {
        format!("Content-Length: {}\r\n", body.len())
    };
    let mut bytes = format!(
        "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\n{framing}{extra}Connection: close\r\n\r\n"
    )
    .into_bytes();
    if chunked {
        for chunk in body.chunks(509) {
            bytes.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            bytes.extend_from_slice(chunk);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"0\r\n\r\n");
    } else {
        bytes.extend_from_slice(body);
    }
    bytes
}
