//! 読み取りを続ける子プロセスと、その確実な回収。
use super::conditions::{query, search_options};
use meibo_core::types::Filters;
use meibo_core::{Engine, Integrity, OpenOptions};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;

pub(super) fn run(args: &[String]) -> crate::Result<i32> {
    if args.len() != 2 || args[0] != "--db" {
        return Err("使い方: bench-reader --db <path>".into());
    }
    read_loop(Path::new(&args[1]))?;
    Ok(0)
}

fn read_loop(path: &Path) -> Result<()> {
    let engine = Engine::open(
        path,
        &OpenOptions {
            integrity: Integrity::Never,
            ..OpenOptions::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    thread::spawn(move || {
        let mut byte = [0];
        while std::io::stdin().read(&mut byte).is_ok_and(|n| n != 0) {}
        flag.store(true, Ordering::Release);
    });
    let mut iterations = 0_u64;
    loop {
        engine
            .search_name("テック", &search_options(false))
            .map_err(|e| e.to_string())?;
        engine
            .list(&query(Filters::default()))
            .map_err(|e| e.to_string())?;
        iterations += 1;
        if iterations == 1 {
            println!("BENCH_READY");
            std::io::stdout().flush().map_err(|e| e.to_string())?;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
    }
    println!("BENCH_ITERATIONS={iterations}");
    Ok(())
}

pub(super) struct Reader {
    child: Child,
    messages: mpsc::Receiver<String>,
    output: Option<thread::JoinHandle<()>>,
}
impl Reader {
    pub(super) fn start(path: &Path) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        #[cfg(not(test))]
        command.arg("bench-reader").arg("--db").arg(path);
        // 単体試験の実行ファイルにも同じ読み取りループを持たせる。
        #[cfg(test)]
        command
            .args([
                "--exact",
                "bench::reader::tests::reader_child_entry",
                "--nocapture",
            ])
            .env("MEIBO_BENCH_READER_DB", path);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().ok_or("reader stdoutなし")?;
        let (sender, messages) = mpsc::channel();
        let output = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let reader = Self {
            child,
            messages,
            output: Some(output),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let line = reader
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| format!("reader準備失敗: {e}"))?;
            if line.ends_with("BENCH_READY") {
                return Ok(reader);
            }
        }
    }
    pub(super) fn finish(mut self) -> Result<u64> {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err(format!("reader終了失敗: {status}"));
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err("reader終了待ちが時間切れです".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        if let Some(output) = self.output.take() {
            output.join().map_err(|_| "reader出力スレッド異常")?;
        }
        self.messages
            .try_iter()
            .find_map(|s| {
                s.strip_prefix("BENCH_ITERATIONS=")
                    .and_then(|n| n.parse().ok())
            })
            .filter(|n| *n > 0)
            .ok_or_else(|| "reader反復回数なし".into())
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(output) = self.output.take() {
            let _ = output.join();
        }
    }
}

#[cfg(test)]
mod tests {
    // REQ 5.6
    #[test]
    fn reader_child_entry() {
        if let Some(path) = std::env::var_os("MEIBO_BENCH_READER_DB") {
            super::read_loop(std::path::Path::new(&path)).unwrap();
        }
    }
}
