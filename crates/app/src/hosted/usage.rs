//! Usage counts and cost estimates for the remote-storage service
//! (usage-dashboard). Only route groups and counts are kept: never a user,
//! a tenant, a query string, a search term, an address or a session.
use std::{collections::BTreeMap, time::Duration};

use meibo_db::{Connection, RemoteUsage, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, json};

use super::HostedResult;

/// Created by `init`, by the restore target and by `usage-enable`.
pub(super) const DDL: &str = "CREATE TABLE IF NOT EXISTS usage_daily(
    day TEXT NOT NULL,
    route TEXT NOT NULL,
    requests INTEGER NOT NULL DEFAULT 0,
    ms INTEGER NOT NULL DEFAULT 0,
    round_trips INTEGER NOT NULL DEFAULT 0,
    rows_read INTEGER NOT NULL DEFAULT 0,
    rows_written INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(day,route)
);";
pub(super) const PRESENT: &str =
    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='usage_daily')";
const READ: &str = "SELECT day,route,requests,ms,round_trips,rows_read,rows_written
    FROM usage_daily WHERE day>=?1 ORDER BY day,route LIMIT 2000";
/// Pending counts are written at most once per this many seconds (1.3).
const FLUSH_SECONDS: i64 = 60;
/// Rows per INSERT; one statement covers every pending row in normal use.
const ROWS_PER_INSERT: usize = 64;
pub(super) const CAP_ENV: &str = "MEIBO_USAGE_CAP_JPY";

/// Prices and free tiers in one place (3.5). Update `CHECKED` with them.
pub(super) mod prices {
    pub(in crate::hosted) const CHECKED: &str = "2026-11-26";
    /// Cloud Run, request-based billing, one vCPU and 0.5 GiB per instance.
    pub(in crate::hosted) const VCPU: f64 = 1.0;
    pub(in crate::hosted) const GIB: f64 = 0.5;
    pub(in crate::hosted) const USD_PER_VCPU_SECOND: f64 = 0.000_024;
    pub(in crate::hosted) const USD_PER_GIB_SECOND: f64 = 0.000_002_5;
    pub(in crate::hosted) const USD_PER_MILLION_REQUESTS: f64 = 0.40;
    /// Monthly free tier per billing account.
    pub(in crate::hosted) const FREE_VCPU_SECONDS: f64 = 180_000.0;
    pub(in crate::hosted) const FREE_GIB_SECONDS: f64 = 360_000.0;
    pub(in crate::hosted) const FREE_REQUESTS: f64 = 2_000_000.0;
    /// Turso free plan, monthly.
    pub(in crate::hosted) const FREE_ROWS_READ: f64 = 500_000_000.0;
    pub(in crate::hosted) const FREE_ROWS_WRITTEN: f64 = 10_000_000.0;
    /// A fixed rate for an estimate, not a market rate.
    pub(in crate::hosted) const JPY_PER_USD: f64 = 155.0;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Group {
    Login,
    Ui,
    List,
    Detail,
    Write,
    Plan,
    Transfer,
    Intake,
    Admin,
    Denied,
    Other,
}

const GROUPS: [(Group, &str, &str); 11] = [
    (Group::Login, "login", "ログイン"),
    (Group::Ui, "ui", "画面と部品"),
    (Group::List, "list", "一覧と検索"),
    (Group::Detail, "detail", "取引先の詳細"),
    (Group::Write, "write", "記録と編集"),
    (Group::Plan, "plan", "予定と商談"),
    (Group::Transfer, "transfer", "取り込みと書き出し"),
    (Group::Intake, "intake", "受付"),
    (Group::Admin, "admin", "管理"),
    (Group::Denied, "denied", "拒否"),
    (Group::Other, "other", "その他"),
];

impl Group {
    fn key(self) -> &'static str {
        GROUPS[self as usize].1
    }
    fn label(self) -> &'static str {
        GROUPS[self as usize].2
    }
    fn parse(key: &str) -> Option<Self> {
        GROUPS
            .iter()
            .find(|(_, name, _)| *name == key)
            .map(|(group, ..)| *group)
    }
}

/// Classify by method, path (no query string) and status only.
pub(super) fn classify(method: &str, path: &str, status: u16) -> Group {
    if matches!(status, 401 | 403 | 405 | 413 | 415 | 429) {
        return Group::Denied;
    }
    let get = method == "GET";
    let company = path
        .strip_prefix("/api/companies/")
        .or_else(|| path.strip_prefix("/api/v1/companies/"));
    match path {
        "/login" | "/auth/login" | "/auth/logout" | "/auth/session" | "/demo/enter" => Group::Login,
        "/auth/audit" | "/usage" | "/usage.js" | "/usage.css" | "/api/sales-targets"
        | "/api/lead-form" => Group::Admin,
        "/api/summary" | "/api/companies" | "/api/v1/companies" | "/api/calling-lists" if get => {
            Group::List
        }
        "/api/import" | "/api/export" => Group::Transfer,
        "/" => Group::Ui,
        _ if path.starts_with("/api/admin/") => Group::Admin,
        _ if path.starts_with("/forms/") || path.starts_with("/api/leads") => Group::Intake,
        _ if path.starts_with("/api/activity-sync") || path.starts_with("/oauth/") => {
            Group::Transfer
        }
        _ if path.starts_with("/api/custom-fields/") && path.ends_with("/search") => Group::List,
        _ if path.starts_with("/api/call-candidates") || path.starts_with("/api/calling-lists") => {
            if get {
                Group::List
            } else {
                Group::Write
            }
        }
        _ if path.starts_with("/api/opportunities")
            || path.starts_with("/api/work-items")
            || path.starts_with("/api/followups")
            || path.starts_with("/api/sales-") =>
        {
            Group::Plan
        }
        _ if company.is_some() => {
            if get {
                Group::Detail
            } else {
                Group::Write
            }
        }
        "/api/companies" | "/api/v1/companies" => Group::Write,
        _ if path.starts_with("/api/custom-fields") => {
            if get {
                Group::List
            } else {
                Group::Write
            }
        }
        _ if !path.starts_with("/api/")
            && path.rsplit_once('.').is_some_and(|(_, ext)| {
                matches!(ext, "js" | "css" | "png" | "svg" | "html" | "ico")
            }) =>
        {
            Group::Ui
        }
        _ => Group::Other,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub(super) requests: u64,
    /// Processing time, each request rounded up to 100 ms (the billed unit).
    pub(super) ms: u64,
    pub(super) round_trips: u64,
    pub(super) rows_read: u64,
    pub(super) rows_written: u64,
}

impl Counts {
    fn add(&mut self, other: Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.ms = self.ms.saturating_add(other.ms);
        self.round_trips = self.round_trips.saturating_add(other.round_trips);
        self.rows_read = self.rows_read.saturating_add(other.rows_read);
        self.rows_written = self.rows_written.saturating_add(other.rows_written);
    }
    #[allow(clippy::cast_precision_loss)] // Estimates; counts stay far below 2^52.
    fn seconds(self) -> f64 {
        self.ms as f64 / 1000.0
    }
    #[allow(clippy::cast_precision_loss)]
    fn usd(self) -> f64 {
        let seconds = self.seconds();
        seconds * prices::VCPU * prices::USD_PER_VCPU_SECOND
            + seconds * prices::GIB * prices::USD_PER_GIB_SECOND
            + self.requests as f64 / 1_000_000.0 * prices::USD_PER_MILLION_REQUESTS
    }
    #[allow(clippy::cast_precision_loss)]
    fn usd_after_free(self) -> f64 {
        let seconds = self.seconds();
        (seconds * prices::VCPU - prices::FREE_VCPU_SECONDS).max(0.0) * prices::USD_PER_VCPU_SECOND
            + (seconds * prices::GIB - prices::FREE_GIB_SECONDS).max(0.0)
                * prices::USD_PER_GIB_SECOND
            + (self.requests as f64 - prices::FREE_REQUESTS).max(0.0) / 1_000_000.0
                * prices::USD_PER_MILLION_REQUESTS
    }
    fn json(self) -> Value {
        json!({
            "requests": self.requests,
            "seconds": round(self.seconds(), 1),
            "round_trips": self.round_trips,
            "rows_read": self.rows_read,
            "rows_written": self.rows_written,
            "jpy": round(self.usd() * prices::JPY_PER_USD, 2),
        })
    }
}

fn round(value: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (value * scale).round() / scale
}

/// Whether this service has the table, and the counts not yet written.
pub(super) enum UsageState {
    /// Local storage: the feature does not exist.
    Off,
    /// Remote storage whose control database has no table yet (2.2).
    Missing,
    On(Usage),
}

pub(super) struct Usage {
    pub(super) pending: BTreeMap<(String, Group), Counts>,
    last_flush: Option<i64>,
    cap_jpy: Option<u64>,
}

impl Usage {
    pub(super) const fn new(cap_jpy: Option<u64>) -> Self {
        Self {
            pending: BTreeMap::new(),
            last_flush: None,
            cap_jpy,
        }
    }

    pub(super) fn record(&mut self, at: i64, group: Group, elapsed: Duration, db: RemoteUsage) {
        // The first request starts the 60-second window.
        self.last_flush.get_or_insert(at);
        let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let billed = millis.div_ceil(100).max(1).saturating_mul(100);
        self.pending
            .entry((day(at), group))
            .or_default()
            .add(Counts {
                requests: 1,
                ms: billed,
                round_trips: db.round_trips,
                rows_read: db.rows_read,
                rows_written: db.rows_written,
            });
    }

    pub(super) fn due(&self, at: i64) -> bool {
        !self.pending.is_empty()
            && self
                .last_flush
                .is_some_and(|last| at < last || at - last >= FLUSH_SECONDS)
    }

    /// Add the pending counts with one upsert. The pending counts are dropped
    /// whatever happens: a failure never changes the caller's result (1.4),
    /// and a lost reply is never resent.
    pub(super) fn flush(&mut self, conn: &Connection, at: i64) -> meibo_db::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        self.last_flush = Some(at);
        let rows = pending.into_iter().collect::<Vec<_>>();
        for chunk in rows.chunks(ROWS_PER_INSERT) {
            let values = vec!["(?,?,?,?,?,?,?)"; chunk.len()].join(",");
            let sql = format!(
                "INSERT INTO usage_daily(day,route,requests,ms,round_trips,rows_read,rows_written)
                 VALUES {values} ON CONFLICT(day,route) DO UPDATE SET
                 requests=requests+excluded.requests,ms=ms+excluded.ms,
                 round_trips=round_trips+excluded.round_trips,
                 rows_read=rows_read+excluded.rows_read,
                 rows_written=rows_written+excluded.rows_written"
            );
            let mut params = Vec::with_capacity(chunk.len() * 7);
            for ((day, group), counts) in chunk {
                params.push(SqlValue::Text(day.clone()));
                params.push(SqlValue::Text(group.key().into()));
                for value in [
                    counts.requests,
                    counts.ms,
                    counts.round_trips,
                    counts.rows_read,
                    counts.rows_written,
                ] {
                    params.push(SqlValue::Integer(i64::try_from(value).unwrap_or(i64::MAX)));
                }
            }
            conn.execute(&sql, params_from_iter(params))?;
        }
        Ok(())
    }

    pub(super) fn pending_requests(&self) -> u64 {
        self.pending.values().map(|counts| counts.requests).sum()
    }

    /// Saved and pending counts for this month and the last 31 days (3.1).
    /// It only reads.
    pub(super) fn report(&self, conn: &Connection, at: i64) -> HostedResult<Value> {
        let today = day(at);
        let month = today[..7].to_owned();
        let first_recent = day(at - 30 * 86_400);
        let from = std::cmp::min(format!("{month}-01"), first_recent.clone());
        let mut merged: BTreeMap<(String, Group), Counts> = BTreeMap::new();
        let mut statement = conn.prepare(READ)?;
        let rows = statement.query_map([from.as_str()], |row| {
            let count = |index| {
                row.get::<_, i64>(index)
                    .map(|value| u64::try_from(value).unwrap_or(0))
            };
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                Counts {
                    requests: count(2)?,
                    ms: count(3)?,
                    round_trips: count(4)?,
                    rows_read: count(5)?,
                    rows_written: count(6)?,
                },
            ))
        })?;
        for row in rows {
            let (day, route, counts) = row?;
            // Unknown route keys (a newer version) count as "other".
            let group = Group::parse(&route).unwrap_or(Group::Other);
            merged.entry((day, group)).or_default().add(counts);
        }
        drop(statement);
        for (key, counts) in &self.pending {
            if key.0 >= from {
                merged.entry(key.clone()).or_default().add(*counts);
            }
        }
        let mut month_groups: BTreeMap<Group, Counts> = BTreeMap::new();
        let mut recent_groups: BTreeMap<Group, Counts> = BTreeMap::new();
        let mut days: BTreeMap<String, Counts> = BTreeMap::new();
        let mut total = Counts::default();
        for ((day, group), counts) in &merged {
            if day.starts_with(&month) && *day <= today {
                month_groups.entry(*group).or_default().add(*counts);
                total.add(*counts);
            }
            if *day >= first_recent && *day <= today {
                recent_groups.entry(*group).or_default().add(*counts);
                days.entry(day.clone()).or_default().add(*counts);
            }
        }
        let series = (0..31)
            .rev()
            .map(|back| {
                let key = day(at - back * 86_400);
                let mut value = days.get(&key).copied().unwrap_or_default().json();
                value["day"] = json!(key);
                value
            })
            .collect::<Vec<_>>();
        let jpy = total.usd() * prices::JPY_PER_USD;
        #[allow(clippy::cast_precision_loss)]
        let cap_ratio = self.cap_jpy.map(|cap| round(jpy / cap as f64, 4));
        let free = free_meters(total);
        let mut totals = total.json();
        totals["jpy_after_free"] = json!(round(total.usd_after_free() * prices::JPY_PER_USD, 2));
        totals["cap_ratio"] = json!(cap_ratio);
        Ok(json!({
            "configured": true,
            "today": today,
            "month": month,
            "price_checked": prices::CHECKED,
            "jpy_per_usd": prices::JPY_PER_USD,
            "cap_jpy": self.cap_jpy,
            "unsaved_requests": self.pending_requests(),
            "flush_seconds": FLUSH_SECONDS,
            "totals": totals,
            "free": free,
            "groups": {"month": group_rows(month_groups), "recent": group_rows(recent_groups)},
            "days": series,
        }))
    }
}

fn group_rows(map: BTreeMap<Group, Counts>) -> Vec<Value> {
    let mut items = map.into_iter().collect::<Vec<_>>();
    items.sort_by(|a, b| b.1.usd().total_cmp(&a.1.usd()).then(a.0.cmp(&b.0)));
    items
        .into_iter()
        .map(|(group, counts)| {
            let mut value = counts.json();
            value["route"] = json!(group.key());
            value["label"] = json!(group.label());
            value
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)] // Estimates; counts stay far below 2^52.
fn free_meters(total: Counts) -> Vec<Value> {
    let meter = |key: &str, label: &str, used: f64, limit: f64| json!({"key":key,"label":label,"used":round(used,1),"limit":limit,"ratio":round(used / limit, 4)});
    vec![
        meter(
            "vcpu_seconds",
            "CPU（vCPU秒）",
            total.seconds() * prices::VCPU,
            prices::FREE_VCPU_SECONDS,
        ),
        meter(
            "gib_seconds",
            "メモリ（GiB秒）",
            total.seconds() * prices::GIB,
            prices::FREE_GIB_SECONDS,
        ),
        meter(
            "requests",
            "要求",
            total.requests as f64,
            prices::FREE_REQUESTS,
        ),
        meter(
            "rows_read",
            "保存先の読み取り行",
            total.rows_read as f64,
            prices::FREE_ROWS_READ,
        ),
        meter(
            "rows_written",
            "保存先の書き込み行",
            total.rows_written as f64,
            prices::FREE_ROWS_WRITTEN,
        ),
    ]
}

/// The optional monthly cap in yen, read once at start (3.6).
pub(super) fn cap_from_env() -> HostedResult<Option<u64>> {
    match std::env::var(CAP_ENV) {
        Err(std::env::VarError::NotPresent) => Ok(None),
        Ok(value) if !value.is_empty() => value
            .parse::<u64>()
            .ok()
            .filter(|cap| (1..=1_000_000_000).contains(cap))
            .map(Some)
            .ok_or_else(|| format!("{CAP_ENV}は1以上の整数（円）で指定してください。").into()),
        _ => Err(format!("{CAP_ENV}は1以上の整数（円）で指定してください。").into()),
    }
}

/// The calendar day in Japan (UTC+9), `YYYY-MM-DD`.
pub(super) fn day(at: i64) -> String {
    let days = (at + 9 * 3600).div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub(super) fn is_page(path: &str) -> bool {
    matches!(
        path,
        "/usage" | "/usage.js" | "/usage.css" | "/api/admin/usage"
    )
}

pub(super) fn page(path: &str) -> Option<crate::server::Reply> {
    let (mime, body) = match path {
        "/usage" => (
            "text/html; charset=utf-8",
            include_str!("../../ui/usage.html"),
        ),
        "/usage.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/usage.js"),
        ),
        "/usage.css" => (
            "text/css; charset=utf-8",
            include_str!("../../ui/usage.css"),
        ),
        _ => return None,
    };
    Some(crate::server::Reply::text(mime, body))
}

/// A browser that opens `/usage` without the admin role gets a page, not
/// JSON. The status stays 403 and the caller has already audited it.
pub(super) fn denied_page() -> crate::server::Reply {
    let mut reply = crate::server::Reply::text(
        "text/html; charset=utf-8",
        include_str!("../../ui/usage-denied.html"),
    );
    reply.status = 403;
    reply
}

/// The entries for admins on the business screen (3.8): a button in the top
/// row from 640px, and a link at the end of the app menu at every width, so
/// the search field keeps its width on phones.
pub(super) fn with_entry(html: &str) -> String {
    html.replacen(
        "<div class=\"global-utilities\">",
        "<div class=\"global-utilities\"><a class=\"usage-entry\" href=\"/usage\">利用量と費用</a>",
        1,
    )
    .replacen(
        "<div id=\"app-menu\" class=\"app-menu\" hidden><p>営業ワークスペース</p>",
        "<div id=\"app-menu\" class=\"app-menu\" hidden><p>営業ワークスペース</p><a class=\"usage-menu-link\" href=\"/usage\">利用量と費用の目安</a>",
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_follow_japan_time() {
        // 2026-11-26T14:59:59Z is 23:59:59 in Japan; one second later is the 27th.
        assert_eq!(day(1_795_705_199), "2026-11-26");
        assert_eq!(day(1_795_705_200), "2026-11-27");
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(951_750_000), "2000-02-29");
    }

    #[test]
    fn groups_use_method_path_and_status_only() {
        let cases = [
            ("POST", "/auth/login", 200, Group::Login),
            ("GET", "/login", 200, Group::Login),
            ("GET", "/", 200, Group::Ui),
            ("GET", "/hosted-app.js", 200, Group::Ui),
            ("GET", "/api/companies", 200, Group::List),
            ("GET", "/api/summary", 200, Group::List),
            ("POST", "/api/custom-fields/3/search", 200, Group::List),
            ("GET", "/api/companies/12", 200, Group::Detail),
            ("POST", "/api/companies/12", 200, Group::Write),
            ("POST", "/api/companies", 201, Group::Write),
            ("POST", "/api/companies/12/activities", 200, Group::Write),
            ("GET", "/api/opportunities", 200, Group::Plan),
            ("GET", "/api/followups", 200, Group::Plan),
            ("POST", "/api/import", 200, Group::Transfer),
            ("GET", "/api/export", 200, Group::Transfer),
            ("GET", "/forms/alpha/key", 200, Group::Intake),
            ("GET", "/api/leads", 200, Group::Intake),
            ("GET", "/api/admin/usage", 200, Group::Admin),
            ("GET", "/auth/audit", 200, Group::Admin),
            ("GET", "/api/companies", 401, Group::Denied),
            ("GET", "/api/admin/usage", 403, Group::Denied),
            ("GET", "/api/companies", 429, Group::Denied),
            ("GET", "/health", 200, Group::Other),
            ("GET", "/api/unknown", 404, Group::Other),
        ];
        for (method, path, status, group) in cases {
            assert_eq!(classify(method, path, status), group, "{method} {path}");
        }
        for (group, key, _) in GROUPS {
            assert_eq!(Group::parse(key), Some(group));
            assert_eq!(group.key(), key);
        }
    }

    #[test]
    fn records_round_up_to_billed_units_and_flush_once_a_minute() {
        let mut usage = Usage::new(None);
        let at = 1_795_705_199;
        assert!(!usage.due(at));
        let db = RemoteUsage {
            round_trips: 2,
            rows_read: 5,
            rows_written: 1,
        };
        usage.record(at, Group::List, Duration::from_millis(1), db);
        usage.record(at, Group::List, Duration::from_millis(101), db);
        usage.record(at + 1, Group::List, Duration::ZERO, RemoteUsage::ZERO);
        assert_eq!(
            usage.pending[&("2026-11-26".into(), Group::List)],
            Counts {
                requests: 2,
                ms: 300,
                round_trips: 4,
                rows_read: 10,
                rows_written: 2,
            }
        );
        assert_eq!(usage.pending_requests(), 3);
        assert!(!usage.due(at + 59));
        assert!(usage.due(at + 60));
        assert!(usage.due(at - 1), "a clock that goes back still flushes");
    }

    #[test]
    fn estimates_use_the_listed_prices() {
        let counts = Counts {
            requests: 1_000_000,
            ms: 1_000_000,
            ..Counts::default()
        };
        // 1000 s × (1 × 0.000024 + 0.5 × 0.0000025) + 0.40 = 0.42525 USD.
        assert!((counts.usd() - 0.425_25).abs() < 1e-9);
        assert!(counts.usd_after_free().abs() < 1e-12);
    }
}
