//! 公開デモの自己終了（アイドル・上限時間・保存量）と見張り・自己確認。
use std::{
    io::{ErrorKind, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// 自己確認の間隔（10秒）。
pub(super) const HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(10);
/// 自己確認1回あたりの応答待ち上限（2秒）。
pub(super) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// 自己確認の連続失敗による自己終了の閾値（3回）。
pub(super) const HEALTH_PROBE_FAILURE_THRESHOLD: u32 = 3;
/// 見張りスレッドの確認周期（1秒）。
pub(super) const WATCHDOG_TICK_INTERVAL: Duration = Duration::from_secs(1);
/// 公開デモ待ち受けループの `recv_timeout` 間隔（500ミリ秒）。
pub(super) const DEMO_RECV_TIMEOUT: Duration = Duration::from_millis(500);
/// 保存先の合計サイズを測る周期（2秒）。
pub(super) const STORAGE_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 保存先の合計サイズが上限以上に留まったときの自己終了までの猶予（10秒）。
pub(super) const STORAGE_FULL_GRACE: Duration = Duration::from_secs(10);

/// 公開デモの自己終了理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResetReason {
    /// `/health` 以外の要求が一定時間なかった。
    Idle,
    /// 待ち受け開始からの稼働時間が上限に達した。
    Uptime,
    /// ハートビート停滞または自己確認の連続失敗を検知した。
    Stalled,
    /// 保存先の合計サイズが上限に達した状態が一定時間続いた。
    Storage,
}

impl ResetReason {
    /// 終了前に標準出力へ1行出力する形式を返す。
    pub(super) const fn log_line(self) -> &'static str {
        match self {
            Self::Idle => "demo-reset reason=idle",
            Self::Uptime => "demo-reset reason=uptime",
            Self::Stalled => "demo-reset reason=stalled",
            Self::Storage => "demo-reset reason=storage",
        }
    }

    /// 終了理由に対応するプロセス終了コードを返す。
    pub(super) const fn exit_code(self) -> i32 {
        match self {
            Self::Idle | Self::Uptime | Self::Storage => 0,
            Self::Stalled => 70,
        }
    }
}

/// 自己終了時の出力とプロセス終了を差し替えるための口。
pub(super) trait ExitSink: Send + Sync {
    /// 指定された理由で出力と終了を行う。
    fn exit(&self, reason: ResetReason);
}

/// 本番用：理由の1行を標準出力へフラッシュしてからプロセスを終了する。
pub(super) struct ProcessExit;

impl ExitSink for ProcessExit {
    fn exit(&self, reason: ResetReason) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{}", reason.log_line());
        let _ = stdout.flush();
        std::process::exit(reason.exit_code());
    }
}

/// `Duration` をミリ秒の `u64` に丸めて変換する。
pub(super) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// 開始時刻 `started_at` からの経過ミリ秒を返す。
pub(super) fn elapsed_ms(started_at: &Instant) -> u64 {
    duration_ms(started_at.elapsed())
}

/// アイドル・上限時間・ハートビートをスレッド間で安全に管理する。
#[derive(Debug)]
pub(super) struct DemoLifecycle {
    idle_timeout_ms: u64,
    max_uptime_ms: u64,
    stall_timeout_ms: u64,
    last_heartbeat_ms: AtomicU64,
    last_activity_ms: AtomicU64,
    in_flight_non_health: AtomicBool,
    exited: AtomicBool,
}

impl DemoLifecycle {
    /// 待ち受け開始時点（経過0ミリ秒）のライフサイクル管理を作る。
    pub(super) fn new(
        idle_timeout: Duration,
        max_uptime: Duration,
        stall_timeout: Duration,
    ) -> Self {
        Self {
            idle_timeout_ms: duration_ms(idle_timeout),
            max_uptime_ms: duration_ms(max_uptime),
            stall_timeout_ms: duration_ms(stall_timeout),
            last_heartbeat_ms: AtomicU64::new(0),
            last_activity_ms: AtomicU64::new(0),
            in_flight_non_health: AtomicBool::new(false),
            exited: AtomicBool::new(false),
        }
    }

    /// 待ち受けループが正常に周回している時刻（ミリ秒）を記録する。
    pub(super) fn heartbeat(&self, now_ms: u64) {
        self.last_heartbeat_ms.store(now_ms, Ordering::Release);
    }

    /// 要求処理の開始を記録する。`/health` 以外はアイドルの時計も進める。
    pub(super) fn begin_request(&self, now_ms: u64, is_health: bool) {
        self.last_heartbeat_ms.store(now_ms, Ordering::Release);
        if !is_health {
            self.last_activity_ms.store(now_ms, Ordering::Release);
            self.in_flight_non_health.store(true, Ordering::Release);
        }
    }

    /// 要求処理の完了を記録する。`/health` 以外はアイドルの時計も進める。
    pub(super) fn end_request(&self, now_ms: u64, is_health: bool) {
        self.last_heartbeat_ms.store(now_ms, Ordering::Release);
        if !is_health {
            self.last_activity_ms.store(now_ms, Ordering::Release);
            self.in_flight_non_health.store(false, Ordering::Release);
        }
    }

    /// 待ち受けループ側の上限時間とアイドル時間を判定する。
    pub(super) fn check_main(&self, now_ms: u64) -> Option<ResetReason> {
        if now_ms >= self.max_uptime_ms {
            return Some(ResetReason::Uptime);
        }
        if !self.in_flight_non_health.load(Ordering::Acquire) {
            let last_activity = self.last_activity_ms.load(Ordering::Acquire);
            if now_ms.saturating_sub(last_activity) >= self.idle_timeout_ms {
                return Some(ResetReason::Idle);
            }
        }
        None
    }

    /// 見張り側のハートビート停滞を判定する。
    pub(super) fn check_stall(&self, now_ms: u64) -> Option<ResetReason> {
        let last_heartbeat = self.last_heartbeat_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(last_heartbeat) >= self.stall_timeout_ms {
            return Some(ResetReason::Stalled);
        }
        None
    }

    /// 最初の1回だけ `sink` を呼んで終了処理を実行する。
    pub(super) fn trigger_exit(&self, reason: ResetReason, sink: &dyn ExitSink) -> bool {
        if self
            .exited
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            sink.exit(reason);
            true
        } else {
            false
        }
    }
}

/// 保存先の上限超過が一定時間続いたかを定期測定で追跡する。
#[derive(Debug)]
pub(super) struct StorageFullTracker {
    limit_bytes: u64,
    poll_interval_ms: u64,
    grace_ms: u64,
    last_poll_ms: Option<u64>,
    full_since_ms: Option<u64>,
}

impl StorageFullTracker {
    /// 上限サイズ・測定周期・連続猶予時間を指定して追跡器を作る。
    pub(super) fn new(limit_bytes: u64, poll_interval: Duration, grace: Duration) -> Self {
        Self {
            limit_bytes,
            poll_interval_ms: duration_ms(poll_interval),
            grace_ms: duration_ms(grace),
            last_poll_ms: None,
            full_since_ms: None,
        }
    }

    /// 測定周期ごとに保存先の大きさを確認し、上限以上が猶予時間以上続いたら
    /// `Some(ResetReason::Storage)` を返す。測定失敗は上限未満として扱う。
    pub(super) fn check<E>(
        &mut self,
        now_ms: u64,
        measure: impl FnOnce() -> Result<u64, E>,
    ) -> Option<ResetReason> {
        if self
            .last_poll_ms
            .is_some_and(|prev| now_ms.saturating_sub(prev) < self.poll_interval_ms)
        {
            return None;
        }
        self.last_poll_ms = Some(now_ms);
        if measure().is_ok_and(|bytes| bytes >= self.limit_bytes) {
            let since = *self.full_since_ms.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) >= self.grace_ms {
                return Some(ResetReason::Storage);
            }
        } else {
            self.full_since_ms = None;
        }
        None
    }
}

/// 自己確認（`GET /health`）の連続失敗回数を追跡する。
#[derive(Debug)]
pub(super) struct HealthProbeTracker {
    consecutive_failures: u32,
    threshold: u32,
}

impl HealthProbeTracker {
    /// 指定回数の連続失敗で `ResetReason::Stalled` を返す追跡器を作る。
    pub(super) const fn new(threshold: u32) -> Self {
        Self {
            consecutive_failures: 0,
            threshold,
        }
    }

    /// 自己確認の成否を記録し、閾値に達したら `Some(ResetReason::Stalled)` を返す。
    pub(super) fn record(&mut self, healthy: bool) -> Option<ResetReason> {
        if healthy {
            self.consecutive_failures = 0;
            None
        } else {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            if self.consecutive_failures >= self.threshold {
                Some(ResetReason::Stalled)
            } else {
                None
            }
        }
    }
}

/// 見張りスレッドの1周分の状態と判定。
#[derive(Debug)]
pub(super) struct WatchdogState {
    probe_interval_ms: u64,
    last_probe_ms: u64,
    probe_tracker: HealthProbeTracker,
}

impl WatchdogState {
    /// 自己確認の間隔と連続失敗の閾値を指定して見張り状態を作る。
    pub(super) fn new(probe_interval: Duration, failure_threshold: u32) -> Self {
        Self {
            probe_interval_ms: duration_ms(probe_interval),
            last_probe_ms: 0,
            probe_tracker: HealthProbeTracker::new(failure_threshold),
        }
    }

    /// ハートビート停滞と定期の自己確認を行い、異常時は `sink` で終了する。
    pub(super) fn tick(
        &mut self,
        lifecycle: &DemoLifecycle,
        now_ms: u64,
        probe: impl FnOnce() -> bool,
        sink: &dyn ExitSink,
    ) -> Option<ResetReason> {
        if let Some(reason) = lifecycle.check_stall(now_ms) {
            lifecycle.trigger_exit(reason, sink);
            return Some(reason);
        }
        if now_ms.saturating_sub(self.last_probe_ms) >= self.probe_interval_ms {
            self.last_probe_ms = now_ms;
            if let Some(reason) = self.probe_tracker.record(probe()) {
                lifecycle.trigger_exit(reason, sink);
                return Some(reason);
            }
        }
        None
    }
}

/// バインド済みアドレスから自己確認用のループバック接続先を組み立てる。
pub(super) const fn loopback_probe_addr(bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V6(v6) if v6.is_loopback() => SocketAddr::new(IpAddr::V6(v6), bound.port()),
        _ => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound.port()),
    }
}

/// 自分の待ち受けポートへ `GET /health` を送り、制限時間内に HTTP 200 が返るか確かめる。
pub(super) fn probe_health(addr: SocketAddr, host: &str, timeout: Duration) -> bool {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return false;
    };
    if remaining.is_zero() || stream.set_write_timeout(Some(remaining)).is_err() {
        return false;
    }
    let request = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0_u8; 256];
    let mut len = 0;
    while len < buf.len() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return false;
        }
        match stream.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => {
                len += n;
                if buf[..len].windows(2).any(|window| window == b"\r\n") {
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    let Ok(text) = std::str::from_utf8(&buf[..len]) else {
        return false;
    };
    let Some(status_line) = text.split("\r\n").next() else {
        return false;
    };
    let mut parts = status_line.split_whitespace();
    let version_ok = parts
        .next()
        .is_some_and(|version| matches!(version, "HTTP/1.1" | "HTTP/1.0"));
    let code_ok = parts.next() == Some("200");
    version_ok && code_ok
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
        thread,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingExit {
        reasons: Mutex<Vec<ResetReason>>,
    }

    impl RecordingExit {
        fn recorded(&self) -> Vec<ResetReason> {
            self.reasons.lock().unwrap().clone()
        }
    }

    impl ExitSink for RecordingExit {
        fn exit(&self, reason: ResetReason) {
            self.reasons.lock().unwrap().push(reason);
        }
    }

    #[test]
    fn reset_reasons_map_to_expected_exit_codes_and_log_lines() {
        assert_eq!(ResetReason::Idle.exit_code(), 0);
        assert_eq!(ResetReason::Idle.log_line(), "demo-reset reason=idle");
        assert_eq!(ResetReason::Uptime.exit_code(), 0);
        assert_eq!(ResetReason::Uptime.log_line(), "demo-reset reason=uptime");
        assert_eq!(ResetReason::Stalled.exit_code(), 70);
        assert_eq!(ResetReason::Stalled.log_line(), "demo-reset reason=stalled");
        assert_eq!(ResetReason::Storage.exit_code(), 0);
        assert_eq!(ResetReason::Storage.log_line(), "demo-reset reason=storage");
    }

    #[test]
    fn storage_full_tracker_requires_ten_continuous_seconds_resets_on_drop_and_ignores_errors() {
        const LIMIT: u64 = 32 * 1024 * 1024;
        let mut tracker = StorageFullTracker::new(LIMIT, STORAGE_POLL_INTERVAL, STORAGE_FULL_GRACE);
        let mut measure_calls = 0_u32;

        // 2秒ごとの測定を繰り返し、9秒（8秒測定 + 9秒スキップ）では終了せず、10秒で Storage を返す。
        for ms in [0_u64, 1_000, 2_000, 4_000, 6_000, 8_000, 9_000] {
            let outcome = tracker.check(ms, || -> Result<u64, &'static str> {
                measure_calls += 1;
                Ok(LIMIT)
            });
            assert_eq!(outcome, None, "should not exit at {ms}ms");
        }
        assert_eq!(measure_calls, 5, "2秒周期でだけ測定される");
        assert_eq!(
            tracker.check(10_000, || -> Result<u64, &'static str> {
                measure_calls += 1;
                Ok(LIMIT)
            }),
            Some(ResetReason::Storage)
        );
        assert_eq!(measure_calls, 6);

        // 途中で上限を下回ると数え直しになる。
        let mut drop_tracker =
            StorageFullTracker::new(LIMIT, STORAGE_POLL_INTERVAL, STORAGE_FULL_GRACE);
        for (ms, bytes) in [
            (0, LIMIT),
            (2_000, LIMIT),
            (4_000, LIMIT),
            (6_000, LIMIT - 1),
            (8_000, LIMIT),
            (10_000, LIMIT),
            (12_000, LIMIT),
            (14_000, LIMIT),
            (16_000, LIMIT),
            (17_000, LIMIT),
        ] {
            assert_eq!(
                drop_tracker.check(ms, || -> Result<u64, &'static str> { Ok(bytes) }),
                None,
                "drop case should not exit at {ms}ms"
            );
        }
        assert_eq!(
            drop_tracker.check(18_000, || -> Result<u64, &'static str> { Ok(LIMIT) }),
            Some(ResetReason::Storage)
        );

        // 測定の失敗は上限未満として扱い、終了せず数え直しにもなる。
        let mut error_tracker =
            StorageFullTracker::new(LIMIT, STORAGE_POLL_INTERVAL, STORAGE_FULL_GRACE);
        for ms in [0_u64, 2_000, 4_000, 6_000, 8_000] {
            assert_eq!(
                error_tracker.check(ms, || -> Result<u64, &'static str> { Ok(LIMIT) }),
                None
            );
        }
        assert_eq!(
            error_tracker.check(10_000, || -> Result<u64, &'static str> { Err("io error") }),
            None
        );
        for ms in [12_000_u64, 14_000, 16_000, 18_000, 20_000] {
            assert_eq!(
                error_tracker.check(ms, || -> Result<u64, &'static str> { Ok(LIMIT) }),
                None,
                "after error should restart grace at {ms}ms"
            );
        }
        assert_eq!(
            error_tracker.check(22_000, || -> Result<u64, &'static str> { Ok(LIMIT) }),
            Some(ResetReason::Storage)
        );

        // 失敗し続けても終了しない。
        let mut always_err =
            StorageFullTracker::new(LIMIT, STORAGE_POLL_INTERVAL, STORAGE_FULL_GRACE);
        for sec in (0_u64..=30).step_by(2) {
            assert_eq!(
                always_err.check(sec * 1_000, || -> Result<u64, &'static str> { Err("io") }),
                None
            );
        }
    }

    #[test]
    fn idle_ignores_health_requests_advances_on_other_requests_and_respects_in_flight() {
        let lifecycle = DemoLifecycle::new(
            Duration::from_secs(10),
            Duration::from_secs(600),
            Duration::from_secs(5),
        );
        let sink = RecordingExit::default();

        // /health を送り続けてもアイドルの時計は進まない。
        for sec in [2, 5, 8, 9] {
            let ms = sec * 1_000;
            lifecycle.begin_request(ms, true);
            lifecycle.end_request(ms + 10, true);
            assert_eq!(lifecycle.check_main(ms + 10), None);
        }
        assert_eq!(lifecycle.check_main(10_000), Some(ResetReason::Idle));

        // 通常要求でアイドルの時計が進み、処理中（本文読み込み途中）はアイドルにならない。
        let active = DemoLifecycle::new(
            Duration::from_secs(10),
            Duration::from_secs(600),
            Duration::from_secs(5),
        );
        active.begin_request(6_000, false);
        assert_eq!(active.check_main(20_000), None);
        active.end_request(8_000, false);
        assert_eq!(active.check_main(17_999), None);
        assert_eq!(active.check_main(18_000), Some(ResetReason::Idle));

        assert!(active.trigger_exit(ResetReason::Idle, &sink));
        assert!(!active.trigger_exit(ResetReason::Uptime, &sink));
        assert_eq!(sink.recorded(), vec![ResetReason::Idle]);
    }

    #[test]
    fn uptime_triggers_even_when_requests_keep_arriving() {
        let lifecycle = DemoLifecycle::new(
            Duration::from_secs(10),
            Duration::from_secs(20),
            Duration::from_secs(5),
        );
        let sink = RecordingExit::default();
        for sec in [5, 10, 15, 19] {
            let ms = sec * 1_000;
            lifecycle.begin_request(ms, false);
            lifecycle.end_request(ms + 50, false);
            assert_eq!(lifecycle.check_main(ms + 50), None);
        }
        lifecycle.begin_request(19_900, false);
        assert_eq!(lifecycle.check_main(20_000), Some(ResetReason::Uptime));
        assert!(lifecycle.trigger_exit(ResetReason::Uptime, &sink));
        assert_eq!(sink.recorded(), vec![ResetReason::Uptime]);
    }

    #[test]
    fn watchdog_triggers_stalled_when_heartbeat_stops_and_stays_alive_when_updated() {
        let lifecycle = DemoLifecycle::new(
            Duration::from_secs(900),
            Duration::from_secs(21_600),
            Duration::from_secs(5),
        );
        let mut watchdog =
            WatchdogState::new(HEALTH_PROBE_INTERVAL, HEALTH_PROBE_FAILURE_THRESHOLD);
        let sink = RecordingExit::default();

        // ハートビートを更新し続ける間は終了しない。
        for sec in 1..=9 {
            let ms = sec * 1_000;
            lifecycle.heartbeat(ms);
            assert_eq!(watchdog.tick(&lifecycle, ms, || true, &sink), None);
        }
        assert_eq!(sink.recorded(), Vec::new());

        // 9秒時点でハートビートが止まると、5秒後（14秒）に stalled で終了する。
        assert_eq!(watchdog.tick(&lifecycle, 13_999, || true, &sink), None);
        assert_eq!(
            watchdog.tick(&lifecycle, 14_000, || true, &sink),
            Some(ResetReason::Stalled)
        );
        assert_eq!(sink.recorded(), vec![ResetReason::Stalled]);
    }

    #[test]
    fn self_probe_exits_after_three_consecutive_failures_and_resets_on_success() {
        let lifecycle = DemoLifecycle::new(
            Duration::from_secs(900),
            Duration::from_secs(21_600),
            Duration::from_secs(60),
        );
        let mut watchdog =
            WatchdogState::new(HEALTH_PROBE_INTERVAL, HEALTH_PROBE_FAILURE_THRESHOLD);
        let sink = RecordingExit::default();
        let mut probe_calls = 0_u32;

        // 2回失敗したあと1回成功すると数え直しになる。
        let schedule = [
            (10_000, false, None),
            (20_000, false, None),
            (30_000, true, None),
            (40_000, false, None),
            (50_000, false, None),
            (60_000, false, Some(ResetReason::Stalled)),
        ];
        for (ms, healthy, expected) in schedule {
            lifecycle.heartbeat(ms);
            let outcome = watchdog.tick(
                &lifecycle,
                ms,
                || {
                    probe_calls += 1;
                    healthy
                },
                &sink,
            );
            assert_eq!(outcome, expected, "unexpected outcome at {ms}ms");
        }
        assert_eq!(probe_calls, 6);
        assert_eq!(sink.recorded(), vec![ResetReason::Stalled]);
    }

    #[test]
    fn probe_health_checks_host_header_200_status_and_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            // 1回目: Hostを検証して200を返す。
            let (mut s1, _) = listener.accept().unwrap();
            let mut req = [0_u8; 512];
            let n = s1.read(&mut req).unwrap();
            let text = String::from_utf8_lossy(&req[..n]);
            assert!(text.starts_with("GET /health HTTP/1.1\r\n"));
            assert!(text.contains("Host: demo.example.test\r\n"));
            s1.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}")
                .unwrap();

            // 2回目: 503を返す。
            let (mut s2, _) = listener.accept().unwrap();
            let _ = s2.read(&mut req);
            s2.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .unwrap();

            // 3回目: 応答を返さずタイムアウトさせる。
            let (_s3, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(250));
        });

        assert!(probe_health(
            addr,
            "demo.example.test",
            Duration::from_secs(1)
        ));
        assert!(!probe_health(
            addr,
            "demo.example.test",
            Duration::from_secs(1)
        ));
        assert!(!probe_health(
            addr,
            "demo.example.test",
            Duration::from_millis(100)
        ));
        handle.join().unwrap();
    }
}
