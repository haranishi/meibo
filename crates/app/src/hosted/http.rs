use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::SocketAddr,
    path::Path,
};

use meibo_core::Engine;
use serde_json::{Value, json};
use tiny_http::{Header, Request, Response, Server, StatusCode};

use super::{
    HostedResult, Identity, LoginOutcome, Role, Store,
    control::ControlStore,
    now,
    store::{MaintenanceGuard, engine_options},
};
use crate::server::leads;
use crate::server::{Input, Reply};

const MAX_JSON: usize = 1024 * 1024;
const MAX_IMPORT: usize = 32 * 1024 * 1024;
const COOKIE: &str = "__Host-meibo-session";

#[derive(Clone, Copy)]
pub(super) struct RequestTime {
    pub(super) at: i64,
    #[cfg(feature = "cloud")]
    pub(super) start: std::time::Duration,
    #[cfg(feature = "cloud")]
    pub(super) started: std::time::Instant,
}

impl RequestTime {
    pub(super) fn new(at: i64) -> Self {
        Self {
            at,
            #[cfg(feature = "cloud")]
            start: std::time::Duration::from_secs(at.try_into().unwrap_or(0)),
            #[cfg(feature = "cloud")]
            started: std::time::Instant::now(),
        }
    }

    fn now() -> HostedResult<Self> {
        let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        Ok(Self {
            at: i64::try_from(epoch.as_secs())?,
            #[cfg(feature = "cloud")]
            start: epoch,
            #[cfg(feature = "cloud")]
            started: std::time::Instant::now(),
        })
    }

    #[cfg(feature = "cloud")]
    pub(super) fn current(self) -> std::time::Duration {
        self.start.saturating_add(self.started.elapsed())
    }
}

#[cfg(all(test, feature = "cloud"))]
mod access_tests;

pub(super) struct HostedConfig {
    pub(super) origin: String,
    pub(super) host: String,
    pub(super) bind: SocketAddr,
    pub(super) development: bool,
    #[cfg(feature = "cloud")]
    pub(super) access: Option<super::access::Access>,
    pub(super) demo: Option<super::demo::DemoConfig>,
}
impl HostedConfig {
    pub(super) fn parse(origin: &str, bind: &str, development: bool) -> HostedResult<Self> {
        let bind: SocketAddr = bind
            .parse()
            .map_err(|_| "bindはIPアドレス:portで指定してください。")?;
        let (scheme, authority) = origin
            .split_once("://")
            .ok_or("originはhttps://domainで指定してください。")?;
        if authority.is_empty()
            || authority.len() > 253
            || authority.contains(['/', '?', '#', '@', '\\'])
            || !authority
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:-".contains(&b))
        {
            return Err("originは小文字のdomainと任意のportだけで指定してください。".into());
        }
        let domain = authority.split(':').next().unwrap_or_default();
        if domain.is_empty()
            || domain
                .split('.')
                .any(|part| part.is_empty() || part.starts_with('-') || part.ends_with('-'))
        {
            return Err("originのdomainを確認してください。".into());
        }
        if let Some((_, port)) = authority.split_once(':')
            && !port.parse::<u16>().is_ok_and(|p| p > 0)
        {
            return Err("originのportを確認してください。".into());
        }
        if development {
            if scheme != "http"
                || !bind.ip().is_loopback()
                || !matches!(domain, "localhost" | "127.0.0.1")
            {
                return Err("HTTP試験は明示したloopbackのoriginとbindに限定されています。".into());
            }
        } else if scheme != "https" {
            return Err("hostedはHTTPSのoriginが必要です。".into());
        }
        let host = if scheme == "https" {
            authority.strip_suffix(":443").unwrap_or(authority)
        } else {
            authority
        };
        Ok(Self {
            origin: format!("{scheme}://{host}"),
            host: host.into(),
            bind,
            development,
            #[cfg(feature = "cloud")]
            access: None,
            demo: None,
        })
    }
    // The same interface serves the native build without the optional verifier.
    #[cfg_attr(not(feature = "cloud"), allow(clippy::unused_self))]
    pub(super) fn access_mode(&self) -> bool {
        #[cfg(feature = "cloud")]
        {
            self.access.is_some()
        }
        #[cfg(not(feature = "cloud"))]
        {
            false
        }
    }
    fn cookie_name(&self) -> &'static str {
        if self.development {
            "meibo-dev-session"
        } else {
            COOKIE
        }
    }
    fn cookie(&self, token: &str, max_age: u32) -> String {
        format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
            self.cookie_name(),
            if self.development { "" } else { "; Secure" }
        )
    }
}

pub(super) struct HostedReply {
    pub(super) reply: Reply,
    headers: Vec<(&'static str, String)>,
    /// `None` keeps the default `no-store`. Only fixed public parts set it.
    cache: Option<&'static str>,
}
impl HostedReply {
    #[cfg(test)]
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
impl From<Reply> for HostedReply {
    fn from(reply: Reply) -> Self {
        Self {
            reply,
            headers: Vec::new(),
            cache: None,
        }
    }
}
fn error(status: u16, message: &'static str) -> HostedReply {
    Reply::error(status, message).into()
}
fn path(input: &Input) -> &str {
    input.target.split('?').next().unwrap_or_default()
}
fn limit(input: &Input) -> usize {
    match path(input) {
        "/auth/login" => 8192,
        "/demo/enter" => 256,
        route if route.starts_with("/forms/") => leads::FORM_LIMIT,
        "/api/import" => MAX_IMPORT,
        _ => MAX_JSON,
    }
}

fn read_only_post(input: &Input) -> bool {
    let Some(id) = input
        .target
        .strip_prefix("/api/custom-fields/")
        .and_then(|rest| rest.strip_suffix("/search"))
    else {
        return false;
    };
    input.method == "POST"
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && id.parse::<i64>().is_ok_and(|value| value > 0)
}

pub(super) trait HostedStorage: std::ops::DerefMut<Target = ControlStore> {
    type Guard;
    fn acquire(&self) -> HostedResult<Self::Guard>;
    fn complete<T>(
        &self,
        guard: Self::Guard,
        result: HostedResult<T>,
        uncertain: bool,
    ) -> HostedResult<T>;
    fn open_engine(&self, tenant: &str) -> HostedResult<Engine>;
    fn storage_root(&self) -> Option<&Path> {
        None
    }
    fn remote(&self) -> bool {
        false
    }
    fn poisoned(&self) -> bool {
        false
    }
    /// Confirm in one round trip that the gate is free and the control store
    /// is ready with this process's identity. Only remote storage uses it.
    #[cfg(feature = "cloud")]
    fn read_ready(&self) -> HostedResult<bool> {
        Ok(false)
    }
}
impl HostedStorage for Store {
    type Guard = MaintenanceGuard;
    fn acquire(&self) -> HostedResult<Self::Guard> {
        self.lock()
    }
    fn complete<T>(
        &self,
        guard: Self::Guard,
        result: HostedResult<T>,
        _uncertain: bool,
    ) -> HostedResult<T> {
        drop(guard);
        result
    }
    fn open_engine(&self, tenant: &str) -> HostedResult<Engine> {
        Ok(Engine::open(&self.tenant_path(tenant)?, &engine_options())?)
    }
    fn storage_root(&self) -> Option<&Path> {
        Some(&self.root)
    }
}

#[cfg(feature = "cloud")]
impl HostedStorage for super::remote::store::RemoteStore {
    type Guard = super::remote::operation::RemoteOperation;
    fn acquire(&self) -> HostedResult<Self::Guard> {
        super::remote::operation::RemoteOperation::begin(self, "http.request")
    }
    fn complete<T>(
        &self,
        mut guard: Self::Guard,
        result: HostedResult<T>,
        uncertain: bool,
    ) -> HostedResult<T> {
        guard.observe_uncertainty(uncertain);
        guard.finish(self, result)
    }
    fn open_engine(&self, tenant: &str) -> HostedResult<Engine> {
        self.open_tenant(tenant)
    }
    fn remote(&self) -> bool {
        true
    }
    fn poisoned(&self) -> bool {
        self.conn.is_poisoned()
    }
    fn read_ready(&self) -> HostedResult<bool> {
        super::remote::store::RemoteStore::read_ready(self)
    }
}

pub(super) struct Hosted<S = Store> {
    pub(super) store: S,
    config: HostedConfig,
    engines: VecDeque<(String, Engine)>,
    uncertain: bool,
    /// Held only during a gate-free read; it fences every remote connection.
    #[cfg(feature = "cloud")]
    fence: Option<meibo_db::ReadFence>,
    /// Quota for anonymous requests, public forms and login (3.6).
    anonymous: AnonymousLimit,
    verified: VerifiedSessions,
    /// A gate-free read or a usage save lost its reply. Stop after responding.
    #[cfg(feature = "cloud")]
    exit_after_reply: bool,
    /// Usage counts of the remote service (usage-dashboard).
    #[cfg(feature = "cloud")]
    pub(super) usage: super::usage::UsageState,
    /// Round trips of this request made before a usage flush (1.6).
    #[cfg(feature = "cloud")]
    usage_carry: meibo_db::RemoteUsage,
    demo_enter: super::demo::EnterRateLimit,
    demo_session_token: Option<String>,
}
impl<S: HostedStorage> Hosted<S> {
    pub(super) fn new(mut store: S, config: HostedConfig) -> Self {
        if config.demo.is_some() {
            store.disable_audit();
        }
        Self {
            store,
            config,
            engines: VecDeque::new(),
            uncertain: false,
            #[cfg(feature = "cloud")]
            fence: None,
            anonymous: AnonymousLimit::default(),
            verified: VerifiedSessions::default(),
            #[cfg(feature = "cloud")]
            exit_after_reply: false,
            #[cfg(feature = "cloud")]
            usage: super::usage::UsageState::Off,
            #[cfg(feature = "cloud")]
            usage_carry: meibo_db::RemoteUsage::ZERO,
            demo_enter: super::demo::EnterRateLimit::default(),
            demo_session_token: None,
        }
    }

    fn body_limit(&self, input: &Input) -> usize {
        if self.config.demo.is_some() {
            match path(input) {
                "/demo/enter" => 256,
                _ => super::demo::MAX_POST_BODY_BYTES,
            }
        } else {
            limit(input)
        }
    }

    fn boundary(&self, input: &Input) -> Option<HostedReply> {
        if input.single_header("Host") != Some(self.config.host.as_str()) {
            return Some(error(403, "公開URLからアクセスしてください。"));
        }
        if !input.target.starts_with('/')
            || input.target.starts_with("//")
            || input.target.len() > 8192
            || input.target.bytes().any(|b| b < 0x20 || b == 0x7f)
        {
            return Some(error(400, "URLを確認してください。"));
        }
        if !matches!(input.method.as_str(), "GET" | "POST") {
            return Some(error(405, "この操作には対応していません。"));
        }
        if input.body.len() > self.body_limit(input) {
            return Some(error(413, "送信内容が上限を超えています。"));
        }
        if input.method == "POST" {
            if input.single_header("Origin") != Some(self.config.origin.as_str())
                || input.single_header("X-Meibo-Local") != Some("1")
            {
                return Some(error(403, "この画面から操作してください。"));
            }
            let expected = if path(input) == "/api/import" {
                "text/csv"
            } else {
                "application/json"
            };
            if input
                .single_header("Content-Type")
                .and_then(|v| v.split(';').next())
                .map(str::trim)
                != Some(expected)
            {
                return Some(error(415, "送信形式を確認してください。"));
            }
        }
        None
    }

    fn token<'a>(&self, input: &'a Input) -> Option<&'a str> {
        let cookie = input.single_header("Cookie")?;
        if cookie.len() > 8192 {
            return None;
        }
        let mut found = None;
        for item in cookie.split(';') {
            let Some((key, value)) = item.trim().split_once('=') else {
                continue;
            };
            if key == self.config.cookie_name() {
                if found.is_some() {
                    return None;
                }
                found = Some(value);
            }
        }
        found
    }

    /// Probe at startup and when an admin opens the API after setup.
    /// Local storage never counts. A communication failure stops the start.
    #[cfg(feature = "cloud")]
    pub(super) fn start_usage(&mut self, cap_jpy: Option<u64>) -> HostedResult<()> {
        use super::usage::{PRESENT, Usage, UsageState};
        if !self.store.remote() {
            self.usage = UsageState::Off;
            return Ok(());
        }
        let present = self
            .store
            .conn
            .query_row(PRESENT, [], |row| row.get::<_, bool>(0));
        self.usage = match present {
            Ok(true) => UsageState::On(Usage::new(cap_jpy)),
            Ok(false) => UsageState::Missing(cap_jpy),
            Err(error) => {
                let error: Box<dyn std::error::Error> = Box::new(error);
                if self.store.poisoned() || uncertain_error(error.as_ref()) {
                    return Err(error);
                }
                UsageState::Missing(cap_jpy)
            }
        };
        Ok(())
    }

    pub(super) fn dispatch(&mut self, input: &Input, at: i64) -> HostedReply {
        let time = RequestTime::new(at);
        self.respond(input, time, self.boundary(input))
    }

    fn respond(
        &mut self,
        input: &Input,
        time: RequestTime,
        early: Option<HostedReply>,
    ) -> HostedReply {
        #[cfg(feature = "cloud")]
        if !matches!(self.usage, super::usage::UsageState::Off) {
            // Anything sent between requests belongs to no request.
            let _ = meibo_db::RemoteUsage::take();
            self.usage_carry = meibo_db::RemoteUsage::ZERO;
        }
        #[cfg(feature = "cloud")]
        let rejected_at_boundary = early.is_some();
        let reply = early.unwrap_or_else(|| self.dispatch_counted(input, time));
        #[cfg(feature = "cloud")]
        if let super::usage::UsageState::On(usage) = &mut self.usage {
            let after = meibo_db::RemoteUsage::take();
            let carry = std::mem::replace(&mut self.usage_carry, meibo_db::RemoteUsage::ZERO);
            let db = meibo_db::RemoteUsage {
                round_trips: carry.round_trips.saturating_add(after.round_trips),
                rows_read: carry.rows_read.saturating_add(after.rows_read),
                rows_written: carry.rows_written.saturating_add(after.rows_written),
            };
            let group = if rejected_at_boundary {
                super::usage::Group::Denied
            } else {
                super::usage::classify(&input.method, path(input), reply.reply.status)
            };
            usage.record(time, group, time.started.elapsed(), db);
        }
        reply
    }

    /// Write pending usage on the serialized path only, at most once per
    /// minute (1.3). Its own round trips are not counted (1.6). A failure
    /// drops the counts. Report uncertainty so the caller retains its gate.
    #[cfg(feature = "cloud")]
    fn usage_save_uncertain(&mut self, time: RequestTime) -> bool {
        let super::usage::UsageState::On(usage) = &mut self.usage else {
            return false;
        };
        if self.fence.is_some() || !usage.due(time.current()) {
            return false;
        }
        self.usage_carry = meibo_db::RemoteUsage::take();
        let result = usage.flush(&self.store.conn, time);
        let _ = meibo_db::RemoteUsage::take();
        self.store.poisoned() || result.is_err_and(|error| uncertain_error(&error))
    }

    fn dispatch_counted(&mut self, input: &Input, time: RequestTime) -> HostedReply {
        let at = time.at;
        let remote = self.store.remote();
        if remote && (self.uncertain || self.poisoned()) {
            return unknown();
        }
        let route = path(input);
        // Login and remote public forms use storage without authenticating the cookie.
        let limited = if remote {
            route != "/health"
                && (route == "/auth/login"
                    || leads::form_path(&input.target).is_some()
                    || !self
                        .token(input)
                        .is_some_and(|token| self.verified.contains(token)))
        } else {
            route == "/auth/login" && self.config.demo.is_none()
        };
        if limited && !self.anonymous.allow(at) {
            return error(
                429,
                "アクセスが集中しています。しばらく待ってから操作してください。",
            );
        }
        #[cfg(feature = "cloud")]
        let access_user = match self.access_user(input, at) {
            Ok(user) => user,
            Err(reply) => return reply,
        };
        #[cfg(not(feature = "cloud"))]
        let access_user: Option<String> = None;
        #[cfg(feature = "cloud")]
        if self.store.remote() {
            if let Some(reply) = self.fixed_reply(input) {
                return reply;
            }
            if ((input.method == "GET"
                && path(input) != "/health"
                && !path(input).starts_with("/api/activity-sync"))
                || read_only_post(input))
                && let Some(reply) = self.fenced_read(input, at, access_user.as_deref())
            {
                return reply;
            }
        }
        let guard = match self.store.acquire() {
            Ok(guard) => guard,
            Err(cause) => {
                if self.store.remote() && (self.store.poisoned() || uncertain_error(cause.as_ref()))
                {
                    self.uncertain = true;
                    return unknown();
                }
                return error(503, "保守処理中、または管理者の確認待ちです。");
            }
        };
        let result = self.dispatch_checked(input, at, access_user.as_deref());
        #[cfg(feature = "cloud")]
        let result = match result {
            Ok(reply) if !self.uncertain && !self.poisoned() && self.usage_save_uncertain(time) => {
                // The business reply is final. Dropping the remote guard sends no SQL.
                self.uncertain = true;
                self.exit_after_reply = true;
                return reply;
            }
            result => result,
        };
        let result = self
            .store
            .complete(guard, result, self.uncertain || self.poisoned());
        match result {
            Ok(reply) => reply,
            Err(cause) if self.store.remote() && uncertain_error(cause.as_ref()) => {
                self.uncertain = true;
                unknown()
            }
            Err(_) => error(
                500,
                "処理を完了できませんでした。時間を置いてもう一度操作してください。",
            ),
        }
    }

    /// Fixed public parts need no database and no gate (remote-performance
    /// 3.1). The order mirrors `dispatch_checked`, and `/health` is excluded.
    #[cfg(feature = "cloud")]
    fn fixed_reply(&self, input: &Input) -> Option<HostedReply> {
        if input.method != "GET" || self.config.demo.is_some() {
            return None;
        }
        let path = path(input);
        if path == "/login" {
            return Some(super::public_entry::login(&self.config).into());
        }
        // The OAuth return page keeps no-store and no revalidation.
        if let Some(reply) = crate::server::activity_sync::callback(path) {
            return Some(reply.into());
        }
        let reply = if path.starts_with("/forms/") {
            leads::public_asset(&input.target)?
        } else {
            super::public_entry::asset(path)?
        };
        Some(revalidated(input, reply))
    }

    /// Answer a read without the gate. The fence starts first, so the
    /// readiness check and the whole read may reuse a socket and every
    /// statement that may write is refused before it is sent. A refusal or
    /// any failure not caused by communication returns `None`, and the
    /// request is handled again on the serialized path.
    #[cfg(feature = "cloud")]
    fn fenced_read(
        &mut self,
        input: &Input,
        at: i64,
        access_user: Option<&str>,
    ) -> Option<HostedReply> {
        self.fence = Some(meibo_db::ReadFence::begin());
        let result = match self.store.read_ready() {
            Ok(true) => self.dispatch_checked(input, at, access_user).map(Some),
            Ok(false) => Ok(None),
            Err(cause) => Err(cause),
        };
        let fence = self.fence.take().expect("active read fence");
        if fence.failed()
            || self.poisoned()
            || result
                .as_ref()
                .is_err_and(|cause| uncertain_error(cause.as_ref()))
        {
            return Some(self.stop_after_uncertain_read());
        }
        match result {
            Ok(None) => Some(error(503, "保守処理中、または管理者の確認待ちです。")),
            Ok(Some(reply)) if !fence.tripped() && reply.reply.status != 500 => Some(reply),
            _ => None,
        }
    }

    #[cfg(all(test, feature = "integration-tests"))]
    pub(in crate::hosted) fn exit_requested(&self) -> bool {
        self.exit_after_reply
    }

    /// An unknown outcome on the gate-free path (4.4): reply once, take no
    /// gate, resend nothing, and end the process after this reply.
    #[cfg(feature = "cloud")]
    fn stop_after_uncertain_read(&mut self) -> HostedReply {
        self.uncertain = true;
        self.exit_after_reply = true;
        unknown()
    }

    #[cfg(feature = "cloud")]
    fn access_user(&mut self, input: &Input, at: i64) -> Result<Option<String>, HostedReply> {
        if let Some(access) = &mut self.config.access {
            let public_asset = input.method == "GET"
                && (matches!(
                    path(input),
                    "/health" | "/oauth/activity-return" | "/oauth/activity-return.js"
                ) || matches!(
                    input.target.as_str(),
                    "/forms/intake.js" | "/forms/intake.css"
                ));
            if public_asset || leads::form_path(&input.target).is_some() {
                return Ok(None);
            }
            let Some(token) = input.single_header("Cf-Access-Jwt-Assertion") else {
                return Err(error(401, "Cloudflareでログインし直してください。"));
            };
            return access.verify(token, at).map(Some).map_err(|cause| {
                let status = if cause == super::access::AccessError::Unavailable {
                    503
                } else {
                    401
                };
                error(status, cause.message())
            });
        }
        Ok(None)
    }

    fn dispatch_checked(
        &mut self,
        input: &Input,
        at: i64,
        access_user: Option<&str>,
    ) -> HostedResult<HostedReply> {
        let path = path(input);
        if self.config.demo.is_some() && super::demo::is_disabled_public_path(path) {
            return Ok(error(404, "対象が見つかりません。"));
        }
        if input.method == "GET"
            && let Some(reply) = crate::server::activity_sync::callback(path)
        {
            return Ok(reply.into());
        }
        if path.starts_with("/forms/") {
            return self.public_intake(input, at);
        }
        if input.method == "GET"
            && let Some(reply) = super::public_entry::asset(path)
        {
            return Ok(reply.into());
        }
        match (input.method.as_str(), path) {
            ("GET", "/health") => return Ok(Reply::json(200, &json!({"ok":true})).into()),
            ("GET", "/login") => {
                return Ok(super::public_entry::login(&self.config).into());
            }
            ("POST", "/auth/login") => return self.login(input, at, access_user),
            ("POST", "/demo/enter") => return self.demo_enter(input, at),
            _ => {}
        }
        let token = self.token(input);
        let identity = match token {
            Some(token) => match access_user {
                #[cfg(feature = "cloud")]
                Some(name) => self.store.authenticate_access(token, name, at)?,
                _ => self.store.authenticate(token, at)?,
            },
            None => None,
        };
        if let Some(token) = token {
            self.verified.record(token, identity.is_some());
        }
        let Some(identity) = identity else {
            if path == "/" || (self.store.remote() && path == "/usage") {
                return Ok(self.unauthenticated_root_redirect(token));
            }
            return Ok(error(401, "ログインしてください。"));
        };
        if input.method == "GET" && path == "/auth/session" {
            return Ok(Reply::json(200, &identity_json(&identity)).into());
        }
        let business = path.starts_with("/api/") || path == "/auth/logout" || path == "/auth/audit";
        if business && input.single_header("X-Meibo-Context") != Some(identity.scope.as_str()) {
            return Ok(Reply::json(409,&json!({"error":"利用者または組織が変わりました。ログインし直してください。","code":"context_changed"})).into());
        }
        if input.method == "POST" && path == "/auth/logout" {
            return self.logout(token, &identity);
        }
        if path == "/auth/audit" {
            if input.method != "GET" || identity.role != Role::Admin {
                return self.denied(&identity);
            }
            return Ok(Reply::json(
                200,
                &json!({"items":self.store.audit_rows(Some(&identity.tenant),200)?}),
            )
            .into());
        }
        #[cfg(feature = "cloud")]
        if self.store.remote() && super::usage::is_page(path) {
            return self.usage_route(input, path, &identity, at);
        }
        if input.method == "POST"
            && ((identity.role == Role::Viewer && !read_only_post(input))
                || (path.ends_with("/erase") && identity.role != Role::Admin))
        {
            return self.denied(&identity);
        }
        if self.config.demo.is_some() && super::demo::is_disabled_auth_path(path) {
            return Ok(demo_disabled_reply());
        }
        if let Some(reply) = self.sales_access(input, &identity)? {
            return Ok(reply);
        }
        if path.starts_with("/auth/") {
            return Ok(error(404, "対象が見つかりません。"));
        }
        if input.method == "GET"
            && let Some(reply) = self.authenticated_asset(path)
        {
            return Ok(reply.into());
        }
        if let Some(reply) = self.check_demo_write_limits(input, &identity)? {
            return Ok(reply);
        }
        self.dispatch_crm(input, &identity)
    }

    fn check_demo_write_limits(
        &mut self,
        input: &Input,
        identity: &Identity,
    ) -> HostedResult<Option<HostedReply>> {
        let Some(demo) = self.config.demo.clone() else {
            return Ok(None);
        };
        if input.method != "POST" || read_only_post(input) {
            return Ok(None);
        }
        let root = self
            .store
            .storage_root()
            .ok_or("公開デモの保存先を確認できません。")?;
        if super::demo::storage_bytes(root)? >= demo.max_storage_bytes {
            return Ok(Some(
                Reply::json(
                    507,
                    &json!({
                        "error": "デモの保存量の上限に達しました。しばらくすると初期化されます。",
                        "code": "demo_storage_full"
                    }),
                )
                .into(),
            ));
        }
        if self.demo_create_limit_reached(path(input), &input.body, &identity.tenant, &demo)? {
            return Ok(Some(
                Reply::json(
                    403,
                    &json!({
                        "error": "デモの上限に達しました。しばらくすると初期化されます。",
                        "code": "demo_limit"
                    }),
                )
                .into(),
            ));
        }
        Ok(None)
    }

    fn demo_create_limit_reached(
        &mut self,
        path: &str,
        body: &[u8],
        tenant: &str,
        demo: &super::demo::DemoConfig,
    ) -> HostedResult<bool> {
        if path == "/api/companies" {
            let count = self
                .engine(tenant)?
                .count(&meibo_core::types::Filters {
                    include_excluded: true,
                    ..meibo_core::types::Filters::default()
                })?
                .n;
            return Ok(count >= demo.max_companies);
        }
        if path == "/api/opportunities" {
            let total = self
                .engine(tenant)?
                .crm_opportunities(&meibo_core::crm::OpportunityQuery {
                    company: None,
                    after: None,
                    limit: 1,
                })?
                .total;
            return Ok(total >= demo.max_opportunities);
        }
        if path == "/api/custom-fields" {
            let count = u64::try_from(self.engine(tenant)?.custom_fields()?.len())?;
            return Ok(count >= demo.max_custom_fields);
        }
        if path == "/api/work-items" {
            let total = self
                .engine(tenant)?
                .crm_work_items(&meibo_core::crm::WorkQuery {
                    company: None,
                    kind: None,
                    completed: None,
                    after: None,
                    limit: 1,
                })?
                .total;
            return Ok(total >= demo.max_work_items);
        }
        if path == "/api/calling-lists" {
            let count = u64::try_from(self.engine(tenant)?.calling_lists()?.len())?;
            return Ok(count >= demo.max_calling_lists);
        }
        if is_company_subroute(path, "/api/companies/", "/activities")
            || is_company_subroute(path, "/api/v1/companies/", "/activities")
        {
            let engine = self.engine(tenant)?;
            if engine.activity_count()? >= demo.max_activities {
                return Ok(true);
            }
            let is_absence = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("follow_up")
                        .and_then(Value::as_str)
                        .map(|action| action == "absence")
                })
                .unwrap_or(false);
            if is_absence {
                let work_total = engine
                    .crm_work_items(&meibo_core::crm::WorkQuery {
                        company: None,
                        kind: None,
                        completed: None,
                        after: None,
                        limit: 1,
                    })?
                    .total;
                return Ok(work_total >= demo.max_work_items);
            }
            return Ok(false);
        }
        if is_activity_cancel_route(path) {
            return Ok(self.engine(tenant)?.activity_count()? >= demo.max_activities);
        }
        if is_company_subroute(path, "/api/companies/", "/contacts") {
            let count = self.engine(tenant)?.contact_count()?;
            return Ok(count >= demo.max_contacts);
        }
        Ok(false)
    }

    /// ログアウト。公開デモでは共有sessionを消さず（1人のログアウトで全員を締め出さない）、Cookieを消す応答だけを返す。
    fn logout(&mut self, token: Option<&str>, identity: &Identity) -> HostedResult<HostedReply> {
        if let Some(token) = token {
            self.verified.record(token, false);
        }
        if self.config.demo.is_none() {
            self.store.logout(token.ok_or("sessionがありません。")?)?;
            self.store.audit(
                &identity.username,
                Some(&identity.tenant),
                "logout",
                "success",
            )?;
        }
        let mut reply: HostedReply = Reply::json(200, &json!({"ok":true})).into();
        reply
            .headers
            .push(("Set-Cookie", self.config.cookie("", 0)));
        reply
            .headers
            .push(("Clear-Site-Data", "\"cache\", \"storage\"".into()));
        Ok(reply)
    }

    fn unauthenticated_root_redirect(&self, token: Option<&str>) -> HostedReply {
        let mut reply = error(302, "ログインしてください。");
        let location = if self.config.demo.is_some() && token.is_some_and(|v| !v.is_empty()) {
            "/login?reset=1"
        } else {
            "/login"
        };
        reply.headers.push(("Location", location.into()));
        reply
    }

    fn authenticated_asset(&self, path: &str) -> Option<Reply> {
        if path == "/demo.css" && self.config.demo.is_some() {
            return Some(super::assets::demo_css());
        }
        super::assets::asset(path)
    }

    fn public_intake(&mut self, input: &Input, at: i64) -> HostedResult<HostedReply> {
        if input.method == "GET"
            && let Some(asset) = leads::public_asset(&input.target)
        {
            return Ok(asset.into());
        }
        if let Some((tenant, key)) = leads::form_path(&input.target)
            && self.store.tenant_exists(tenant)?
        {
            return Ok(leads::public_form(self.engine(tenant)?, key, input, at).into());
        }
        Ok(error(404, "この受付フォームは利用できません。"))
    }

    fn sales_access(
        &self,
        input: &Input,
        identity: &Identity,
    ) -> HostedResult<Option<HostedReply>> {
        let path = path(input);
        if path == "/api/lead-form" && identity.role != Role::Admin
            || path == "/api/sales-targets"
                && input.method == "POST"
                && identity.role != Role::Admin
            || identity.role == Role::Viewer
                && (path == "/api/leads"
                    || path.starts_with("/api/leads/")
                    || path.starts_with("/api/activity-sync"))
        {
            return self.denied(identity).map(Some);
        }
        if path == "/api/sales-members" && input.method == "GET" && input.target == path {
            return Ok(Some(Reply::json(200,&json!({"members":self.store.sales_members(&identity.tenant)?,"current_user_id":identity.user_id,"hosted":true})).into()));
        }
        if input.method == "POST"
            && (path == "/api/sales-targets"
                || path == "/api/opportunities"
                || path.starts_with("/api/opportunities/")
                || path.starts_with("/api/leads/"))
            && let Ok(payload) = serde_json::from_slice::<Value>(&input.body)
            && let Some(owner_id) = payload.get("owner_id").and_then(Value::as_i64)
            && !self.store.sales_member(&identity.tenant, owner_id)?
        {
            return Ok(Some(error(
                400,
                "この組織で営業を担当できる利用者を選択してください。",
            )));
        }
        Ok(None)
    }

    fn dispatch_crm(&mut self, input: &Input, identity: &Identity) -> HostedResult<HostedReply> {
        let path = path(input);
        let mut trusted = Input {
            method: input.method.clone(),
            target: input.target.clone(),
            body: input.body.clone(),
            headers: vec![
                ("Host".into(), "127.0.0.1:0".into()),
                ("Origin".into(), "http://127.0.0.1:0".into()),
                ("X-Meibo-Local".into(), "1".into()),
            ],
        };
        if let Some(content) = input.single_header("Content-Type") {
            trusted
                .headers
                .push(("Content-Type".into(), content.into()));
        }
        let mut reply = crate::server::dispatch_for_actor(
            self.engine(&identity.tenant)?,
            &trusted,
            0,
            Some(identity.user_id),
            Some(&identity.scope),
        );
        if self.store.remote() && self.poisoned() {
            return Ok(unknown());
        }
        if input.method == "POST" && !read_only_post(input) {
            self.store.audit(
                &identity.username,
                Some(&identity.tenant),
                "crm.write",
                if reply.status < 400 {
                    "success"
                } else {
                    "failed"
                },
            )?;
        }
        if path == "/" && reply.status == 200 {
            let html = String::from_utf8(reply.body)?;
            let mut html = super::assets::hosted_paths(&html)
                .replace(
                    "<script src=",
                    "<script type=\"application/x-meibo-gated\" data-meibo-src=",
                )
                .replace("<main ", "<main hidden data-meibo-gated ")
                .replace(
                    "</body>",
                    "<script src=\"/login.js\" defer></script></body>",
                );
            if let Some(demo) = &self.config.demo {
                html = super::public_entry::apply_demo_workspace(&html, demo);
            }
            #[cfg(feature = "cloud")]
            if self.store.remote() && identity.role == Role::Admin {
                html = super::usage::with_entry(&html);
            }
            reply.body = html.into_bytes();
        }
        Ok(reply.into())
    }

    /// Admins get the page and data. Everyone else is denied and audited; a
    /// browser opening `/usage` gets a page instead of JSON. The stylesheet
    /// has no data, and the denied page uses it.
    #[cfg(feature = "cloud")]
    fn usage_route(
        &mut self,
        input: &Input,
        path: &str,
        identity: &Identity,
        at: i64,
    ) -> HostedResult<HostedReply> {
        if input.method == "GET" && path == "/usage.css" {
            return self.usage_reply(path, at);
        }
        if input.method != "GET" || identity.role != Role::Admin {
            let mut reply = self.denied(identity)?;
            if input.method == "GET" && path == "/usage" {
                reply.reply = super::usage::denied_page();
            }
            return Ok(reply);
        }
        self.usage_reply(path, at)
    }

    /// The admin usage page and data (usage-dashboard 3.1-3.3). It only reads.
    #[cfg(feature = "cloud")]
    fn usage_reply(&mut self, path: &str, at: i64) -> HostedResult<HostedReply> {
        if let Some(page) = super::usage::page(path) {
            return Ok(page.into());
        }
        if let super::usage::UsageState::Missing(cap_jpy) = self.usage {
            self.start_usage(cap_jpy)?;
        }
        let value = match &mut self.usage {
            super::usage::UsageState::On(usage) => usage.report(&self.store.conn, at)?,
            _ => json!({"configured": false}),
        };
        Ok(Reply::json(200, &value).into())
    }

    fn denied(&self, identity: &Identity) -> HostedResult<HostedReply> {
        self.store.audit(
            &identity.username,
            Some(&identity.tenant),
            "authorization",
            "denied",
        )?;
        Ok(error(403, "この操作を行う権限がありません。"))
    }

    fn demo_enter(&mut self, input: &Input, at: i64) -> HostedResult<HostedReply> {
        let Some(demo) = self.config.demo.as_ref() else {
            return Ok(error(404, "対象が見つかりません。"));
        };
        let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&input.body) else {
            return Ok(error(400, "入力内容を確認してください。"));
        };
        if !map.is_empty() {
            return Ok(error(400, "入力内容を確認してください。"));
        }
        let enter_per_minute = demo.enter_per_minute;
        if !self.demo_enter.allow(at, enter_per_minute) {
            return Ok(Reply::json(
                429,
                &json!({
                    "error": "混み合っています。少し待ってからもう一度お試しください。",
                    "code": "demo_busy"
                }),
            )
            .into());
        }
        let token = match self.demo_session_token.as_deref() {
            Some(token) => token.to_owned(),
            None => super::auth::random_hex(32)?,
        };
        let outcome = self.store.open_demo_session(
            super::demo::DEMO_TENANT,
            super::demo::DEMO_USER,
            &token,
            at,
        )?;
        if matches!(outcome, LoginOutcome::Success(_)) && self.demo_session_token.is_none() {
            self.demo_session_token = Some(token);
        }
        Ok(self.login_response(outcome))
    }

    fn login(
        &mut self,
        input: &Input,
        at: i64,
        access_user: Option<&str>,
    ) -> HostedResult<HostedReply> {
        #[cfg(not(feature = "cloud"))]
        let _ = access_user;
        if self.config.demo.is_some() {
            return Ok(error(
                404,
                "公開デモではパスワードログインは利用できません。",
            ));
        }
        let value: Value = match serde_json::from_slice(&input.body) {
            Ok(v) => v,
            Err(_) => return Ok(error(400, "入力内容を確認してください。")),
        };
        #[cfg(feature = "cloud")]
        if self.config.access_mode() {
            let Some(tenant) = value.get("tenant").and_then(Value::as_str) else {
                return Ok(error(400, "組織IDを確認してください。"));
            };
            if value.as_object().is_none_or(|v| v.len() != 1) {
                return Ok(error(400, "組織IDだけを指定してください。"));
            }
            let Some(name) = access_user else {
                return Ok(error(401, "Cloudflareでログインし直してください。"));
            };
            let outcome = self.store.login_access(tenant, name, at)?;
            return Ok(self.login_response(outcome));
        }
        let Some((tenant, name, password)) = value
            .get("tenant")
            .and_then(Value::as_str)
            .zip(value.get("username").and_then(Value::as_str))
            .zip(value.get("password").and_then(Value::as_str))
            .map(|((t, n), p)| (t, n, p))
        else {
            return Ok(error(400, "入力内容を確認してください。"));
        };
        let outcome = self.store.login(tenant, name, password, at)?;
        Ok(self.login_response(outcome))
    }

    fn login_response(&mut self, outcome: LoginOutcome) -> HostedReply {
        match outcome {
            LoginOutcome::Invalid if self.config.access_mode() => {
                error(403, "このアカウントは指定の組織で利用できません。")
            }
            LoginOutcome::Invalid => error(401, "組織・利用者名・passwordを確認してください。"),
            LoginOutcome::Throttled => error(
                429,
                "ログイン試行の上限です。15分後にもう一度操作してください。",
            ),
            LoginOutcome::Success(login) => {
                if self.store.remote() {
                    self.verified.record(&login.token, true);
                }
                let mut reply: HostedReply =
                    Reply::json(200, &identity_json(&login.identity)).into();
                reply
                    .headers
                    .push(("Set-Cookie", self.config.cookie(&login.token, 8 * 3600)));
                reply
            }
        }
    }

    fn engine(&mut self, tenant: &str) -> HostedResult<&mut Engine> {
        if let Some(pos) = self.engines.iter().position(|(slug, _)| slug == tenant) {
            let entry = self.engines.remove(pos).ok_or("接続を取得できません。")?;
            self.engines.push_back(entry);
        } else {
            let opened = self.store.open_engine(tenant);
            // While fenced, the serialized retry decides; poison still stops.
            #[cfg(feature = "cloud")]
            if self.store.remote()
                && self.fence.is_none()
                && opened.as_ref().is_err_and(|error| {
                    super::remote::operation::failed_construction(error.as_ref())
                })
            {
                self.uncertain = true;
            }
            let engine = opened?;
            if self.engines.len() >= 4 {
                self.engines.pop_front();
            }
            self.engines.push_back((tenant.into(), engine));
        }
        self.engines
            .back_mut()
            .map(|(_, engine)| engine)
            .ok_or_else(|| "接続を取得できません。".into())
    }

    fn poisoned(&self) -> bool {
        #[cfg(feature = "cloud")]
        {
            self.store.poisoned() || self.engines.iter().any(|(_, engine)| engine.is_poisoned())
        }
        #[cfg(not(feature = "cloud"))]
        {
            self.store.poisoned()
        }
    }

    pub(super) fn serve(mut self) -> HostedResult<()> {
        if let Some(demo) = self.config.demo.clone() {
            if self.store.remote() {
                return Err("公開デモは遠隔保存では利用できません。".into());
            }
            return self.serve_demo(&demo);
        }
        #[cfg(feature = "cloud")]
        if self.store.remote() {
            self.start_usage(super::usage::cap_from_env()?)?;
        }
        let server =
            Server::http(self.config.bind).map_err(|_| "hostedサーバーを起動できません。")?;
        println!(
            "hosted listening: {} (public origin: {})",
            server.server_addr(),
            self.config.origin
        );
        std::io::stdout().flush()?;
        for mut request in server.incoming_requests() {
            let reply = self.receive(&mut request, RequestTime::now()?);
            send(request, reply);
            #[cfg(feature = "cloud")]
            if self.exit_after_reply {
                return Err(super::remote::operation::UNKNOWN_MESSAGE.into());
            }
        }
        Ok(())
    }

    pub(super) fn receive(&mut self, request: &mut Request, time: RequestTime) -> HostedReply {
        let mut input = Input {
            method: request.method().as_str().into(),
            target: request.url().into(),
            headers: request
                .headers()
                .iter()
                .map(|h| (h.field.as_str().to_string(), h.value.to_string()))
                .collect(),
            body: Vec::new(),
        };
        let max = limit(&input);
        let early = if let Some(reply) = self.boundary(&input) {
            Some(reply)
        } else if request.body_length().is_some_and(|size| size > max) {
            Some(error(413, "送信内容が上限を超えています。"))
        } else if input.method == "POST"
            && request
                .as_reader()
                .take((max + 1) as u64)
                .read_to_end(&mut input.body)
                .is_err()
        {
            Some(error(400, "送信内容を読み取れません。"))
        } else if input.body.len() > max {
            Some(error(413, "送信内容が上限を超えています。"))
        } else {
            None
        };
        self.respond(&input, time, early)
    }

    fn handle_demo_request(&mut self, mut request: Request) -> HostedResult<()> {
        let mut input = Input {
            method: request.method().as_str().into(),
            target: request.url().into(),
            headers: request
                .headers()
                .iter()
                .map(|h| (h.field.as_str().to_string(), h.value.to_string()))
                .collect(),
            body: Vec::new(),
        };
        // tiny_http 0.12 は Response::add_header の Connection ヘッダーを黙って捨て、
        // 拒否した要求でも Request の破棄（EqualReader::Drop）で宣言長分の本文を読み捨てる。
        // アプリ側から回避できないため、本文を受け切って宣言長と一致する要求だけを渡す前段が前提になる。
        let reply = if let Some(reply) = self.boundary(&input) {
            reply
        } else if let Some((status, message)) = super::demo::check_post_preflight(
            &input.method,
            &input.headers,
            request.body_length(),
            self.body_limit(&input),
        ) {
            error(status, message)
        } else if input.method == "POST"
            && request
                .as_reader()
                .take((self.body_limit(&input) + 1) as u64)
                .read_to_end(&mut input.body)
                .is_err()
        {
            error(400, "送信内容を読み取れません。")
        } else {
            self.dispatch(&input, now()?)
        };
        send(request, reply);
        Ok(())
    }

    fn serve_demo(mut self, demo: &super::demo::DemoConfig) -> HostedResult<()> {
        use std::{sync::Arc, thread, time::Instant};

        use super::demo_lifecycle::{
            DEMO_RECV_TIMEOUT, DemoLifecycle, HEALTH_PROBE_FAILURE_THRESHOLD,
            HEALTH_PROBE_INTERVAL, HEALTH_PROBE_TIMEOUT, ProcessExit, ResetReason,
            STORAGE_FULL_GRACE, STORAGE_POLL_INTERVAL, StorageFullTracker, WATCHDOG_TICK_INTERVAL,
            WatchdogState, elapsed_ms, loopback_probe_addr, probe_health,
        };

        let storage_root = self
            .store
            .storage_root()
            .ok_or("公開デモの保存先を確認できません。")?
            .to_path_buf();
        let server =
            Server::http(self.config.bind).map_err(|_| "hostedサーバーを起動できません。")?;
        let bound_addr = server
            .server_addr()
            .to_ip()
            .ok_or("待ち受けアドレスを取得できません。")?;
        println!(
            "hosted listening: {} (public origin: {})",
            server.server_addr(),
            self.config.origin
        );
        std::io::stdout().flush()?;

        let started_at = Instant::now();
        let lifecycle = Arc::new(DemoLifecycle::new(
            demo.idle_timeout,
            demo.max_uptime,
            demo.stall_timeout,
        ));
        let mut storage_tracker = StorageFullTracker::new(
            demo.max_storage_bytes,
            STORAGE_POLL_INTERVAL,
            STORAGE_FULL_GRACE,
        );
        let watchdog_lifecycle = Arc::clone(&lifecycle);
        let probe_addr = loopback_probe_addr(bound_addr);
        let probe_host = self.config.host.clone();
        thread::Builder::new()
            .name("demo-watchdog".into())
            .spawn(move || {
                let mut watchdog =
                    WatchdogState::new(HEALTH_PROBE_INTERVAL, HEALTH_PROBE_FAILURE_THRESHOLD);
                loop {
                    thread::sleep(WATCHDOG_TICK_INTERVAL);
                    let now_ms = elapsed_ms(&started_at);
                    if watchdog
                        .tick(
                            &watchdog_lifecycle,
                            now_ms,
                            || probe_health(probe_addr, &probe_host, HEALTH_PROBE_TIMEOUT),
                            &ProcessExit,
                        )
                        .is_some()
                    {
                        break;
                    }
                }
            })
            .map_err(|_| "公開デモの見張りスレッドを起動できません。")?;

        loop {
            match server.recv_timeout(DEMO_RECV_TIMEOUT) {
                Ok(Some(request)) => {
                    let is_health = request.method().as_str() == "GET"
                        && request.url().split('?').next() == Some("/health");
                    lifecycle.begin_request(elapsed_ms(&started_at), is_health);
                    self.handle_demo_request(request)?;
                    let finished_ms = elapsed_ms(&started_at);
                    lifecycle.end_request(finished_ms, is_health);
                    if let Some(reason) = lifecycle.check_main(finished_ms).or_else(|| {
                        storage_tracker
                            .check(finished_ms, || super::demo::storage_bytes(&storage_root))
                    }) {
                        lifecycle.trigger_exit(reason, &ProcessExit);
                    }
                }
                Ok(None) => {
                    let now_ms = elapsed_ms(&started_at);
                    lifecycle.heartbeat(now_ms);
                    if let Some(reason) = lifecycle.check_main(now_ms).or_else(|| {
                        storage_tracker.check(now_ms, || super::demo::storage_bytes(&storage_root))
                    }) {
                        lifecycle.trigger_exit(reason, &ProcessExit);
                    }
                }
                Err(_) => {
                    lifecycle.trigger_exit(ResetReason::Stalled, &ProcessExit);
                    return Err("公開デモの待ち受けが停止しました。".into());
                }
            }
        }
    }
}

fn demo_disabled_reply() -> HostedReply {
    Reply::json(
        403,
        &json!({
            "error": "公開デモでは使えません。",
            "code": "demo_disabled"
        }),
    )
    .into()
}

fn is_positive_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|id| id > 0)
}

fn is_company_subroute(path: &str, prefix: &str, suffix: &str) -> bool {
    path.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .is_some_and(is_positive_id)
}

fn is_activity_cancel_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/companies/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    matches!(
        parts.as_slice(),
        [company, "activities", activity, "cancel"]
            if is_positive_id(company) && is_positive_id(activity)
    )
}

fn uncertain_error(error: &(dyn std::error::Error + 'static)) -> bool {
    #[cfg(feature = "cloud")]
    {
        super::remote::operation::is_uncertain(error)
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = error;
        false
    }
}

/// One service-wide token bucket for requests without a verified session:
/// five per second, with at most 100 at once. `at` is the injected clock.
struct AnonymousLimit {
    tokens: i64,
    at: i64,
}
impl AnonymousLimit {
    const RATE: i64 = 5;
    const BURST: i64 = 100;
    fn allow(&mut self, at: i64) -> bool {
        if at > self.at {
            let refill = at.saturating_sub(self.at).saturating_mul(Self::RATE);
            self.tokens = self.tokens.saturating_add(refill).min(Self::BURST);
        }
        // A clock that went back resets the base without refilling, so
        // the bucket recovers from the new time instead of freezing.
        self.at = at;
        if self.tokens > 0 {
            self.tokens -= 1;
            true
        } else {
            false
        }
    }
}
/// Digests of sessions this process authenticated, newest last (3.6).
/// Only these cookies skip the anonymous bucket; a failure removes one.
#[derive(Default)]
struct VerifiedSessions(VecDeque<Vec<u8>>);
impl VerifiedSessions {
    const LIMIT: usize = 64;
    fn contains(&self, token: &str) -> bool {
        let digest = super::auth::digest(token.as_bytes());
        self.0.contains(&digest)
    }
    fn record(&mut self, token: &str, valid: bool) {
        let digest = super::auth::digest(token.as_bytes());
        self.0.retain(|known| *known != digest);
        if valid {
            if self.0.len() == Self::LIMIT {
                self.0.pop_front();
            }
            self.0.push_back(digest);
        }
    }
}

impl Default for AnonymousLimit {
    fn default() -> Self {
        Self {
            tokens: Self::BURST,
            at: i64::MIN,
        }
    }
}

/// Let the browser reuse a fixed part after revalidation. The tag is the
/// content hash, so a new deployment's different part is never reused.
#[cfg(feature = "cloud")]
fn revalidated(input: &Input, reply: Reply) -> HostedReply {
    let digest = super::auth::digest(&reply.body);
    let tag = format!("\"{}\"", super::auth::hex(&digest[..16]));
    let matches = input.single_header("If-None-Match").is_some_and(|value| {
        value.trim() == "*"
            || value
                .split(',')
                .map(str::trim)
                .any(|candidate| candidate.strip_prefix("W/").unwrap_or(candidate) == tag)
    });
    let mut reply: HostedReply = if matches {
        Reply {
            status: 304,
            mime: reply.mime,
            body: Vec::new(),
            attachment: false,
        }
        .into()
    } else {
        reply.into()
    };
    reply.cache = Some("no-cache");
    reply.headers.push(("ETag", tag));
    reply
}

fn unknown() -> HostedReply {
    Reply::json(503, &json!({"code":"outcome_unknown","error":"処理結果を確認できません。再送せず管理者へ連絡してください。"})).into()
}

fn identity_json(identity: &Identity) -> Value {
    json!({"user_id":identity.user_id,"username":identity.username,"tenant":identity.tenant,"role":identity.role.name(),"storage_scope":identity.scope})
}

fn send(request: Request, reply: HostedReply) {
    let mut response =
        Response::from_data(reply.reply.body).with_status_code(StatusCode(reply.reply.status));
    for (name, value) in [
        ("Content-Type", reply.reply.mime),
        ("Cache-Control", reply.cache.unwrap_or("no-store")),
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    ] {
        if let Ok(header) = Header::from_bytes(name, value) {
            response.add_header(header);
        }
    }
    for (name, value) in reply.headers {
        if let Ok(header) = Header::from_bytes(name, value) {
            response.add_header(header);
        }
    }
    if reply.reply.attachment {
        response.add_header(
            Header::from_bytes(
                "Content-Disposition",
                "attachment; filename=\"meibo-companies.csv\"",
            )
            .expect("fixed header"),
        );
    }
    let _ = request.respond(response);
}

#[cfg(test)]
mod anonymous_limit_tests {
    use super::AnonymousLimit;

    #[test]
    fn bursts_to_one_hundred_then_refills_five_per_second() {
        let mut limit = AnonymousLimit::default();
        let start = 1_900_000_000;
        assert!((0..100).all(|_| limit.allow(start)));
        assert!(!limit.allow(start));
        assert!((0..5).all(|_| limit.allow(start + 1)));
        assert!(!limit.allow(start + 1));
        // A clock that goes back never refills, but recovers from then on.
        assert!(!limit.allow(start - 600));
        assert!((0..5).all(|_| limit.allow(start - 599)));
        assert!(!limit.allow(start - 599));
        // A long pause refills only up to the burst.
        assert!((0..100).all(|_| limit.allow(start + 3600)));
        assert!(!limit.allow(start + 3600));
    }
}

#[cfg(all(test, feature = "cloud"))]
mod cache_tests {
    use super::{Input, Reply, revalidated};

    #[test]
    fn if_none_match_uses_weak_comparison() {
        let mut input = Input {
            method: "GET".into(),
            target: "/login.css".into(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let part = || Reply::text("text/css", "body{}");
        let first = revalidated(&input, part());
        let tag = first.header("ETag").unwrap();
        assert_eq!(first.reply.status, 200);
        for (value, expected) in [
            ("*".to_owned(), 304),
            (format!("W/{tag}"), 304),
            (format!("\"other\", {tag}"), 304),
            (format!("\"other,part\",\tW/{tag}, \"last\""), 304),
            ("\"other\", W/\"old\"".to_owned(), 200),
            ("\"with,*,commas\"".to_owned(), 200),
        ] {
            input.headers = vec![("If-None-Match".into(), value)];
            let response = revalidated(&input, part());
            assert_eq!(response.reply.status, expected);
            assert_eq!(response.reply.body.is_empty(), expected == 304);
            assert_eq!(response.header("ETag"), Some(tag));
            assert_eq!(response.cache, Some("no-cache"));
        }
    }
}
