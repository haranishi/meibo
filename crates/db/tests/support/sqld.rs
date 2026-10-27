use std::{
    env, fs, io,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const START_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Sqld {
    child: Child,
    address: SocketAddr,
    directory: tempfile::TempDir,
}

impl Sqld {
    pub fn start() -> io::Result<Self> {
        let root = env::var_os("MEIBO_SQLD_TEST_ROOT")
            .or_else(|| env::var_os("CARGO_TARGET_DIR"))
            .map_or_else(env::temp_dir, PathBuf::from)
            .join("meibo-sqld-fixtures");
        fs::create_dir_all(&root)?;
        let directory = tempfile::Builder::new()
            .prefix("synthetic-")
            .tempdir_in(root)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        drop(listener);

        let child = Command::new(binary())
            .env_clear()
            .env("RUST_LOG", "off")
            .env("TOKIO_WORKER_THREADS", "2")
            .env("TMPDIR", directory.path())
            .current_dir(directory.path())
            .arg("--db-path")
            .arg(directory.path().join("synthetic.sqld"))
            .args(["--http-listen-addr", &address.to_string()])
            .args(["--grpc-listen-addr", "127.0.0.1:0"])
            .args(["--no-welcome", "--disable-metrics"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    "cannot launch sqld; set MEIBO_SQLD_BINARY to a compatible sqld binary",
                )
            })?;
        let mut server = Self {
            child,
            address,
            directory,
        };
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if server.child.try_wait()?.is_some() {
                return Err(io::Error::other("sqld exited before becoming ready"));
            }
            if ready(address) {
                return Ok(server);
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "sqld startup timed out",
        ))
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
}

impl Drop for Sqld {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn binary() -> PathBuf {
    if let Some(binary) = env::var_os("MEIBO_SQLD_BINARY") {
        return PathBuf::from(binary);
    }
    if let Some(home) = env::var_os("HOME") {
        let installed = PathBuf::from(home).join(".turso/sqld");
        if installed.is_file() {
            return installed;
        }
    }
    PathBuf::from("sqld")
}

fn ready(address: SocketAddr) -> bool {
    let timeout = Duration::from_millis(100);
    let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
        || stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .is_err()
    {
        return false;
    }
    let mut prefix = [0; 12];
    stream.read_exact(&mut prefix).is_ok() && &prefix == b"HTTP/1.1 200"
}
