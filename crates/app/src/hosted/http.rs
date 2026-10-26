use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::SocketAddr,
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
use crate::server::{Input, Reply};

const MAX_JSON: usize = 1024 * 1024;
const MAX_IMPORT: usize = 32 * 1024 * 1024;
const COOKIE: &str = "__Host-meibo-session";

pub(super) struct HostedConfig {
    pub(super) origin: String,
    pub(super) host: String,
    pub(super) bind: SocketAddr,
    pub(super) development: bool,
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
        })
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
}
impl From<Reply> for HostedReply {
    fn from(reply: Reply) -> Self {
        Self {
            reply,
            headers: Vec::new(),
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
    fn remote(&self) -> bool {
        false
    }
    fn poisoned(&self) -> bool {
        false
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
}

pub(super) struct Hosted<S = Store> {
    pub(super) store: S,
    config: HostedConfig,
    engines: VecDeque<(String, Engine)>,
    uncertain: bool,
}
impl<S: HostedStorage> Hosted<S> {
    pub(super) fn new(store: S, config: HostedConfig) -> Self {
        Self {
            store,
            config,
            engines: VecDeque::new(),
            uncertain: false,
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
        if input.body.len() > limit(input) {
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

    pub(super) fn dispatch(&mut self, input: &Input, at: i64) -> HostedReply {
        if let Some(reply) = self.boundary(input) {
            return reply;
        }
        if self.store.remote() && (self.uncertain || self.poisoned()) {
            return unknown();
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
        let result = self.dispatch_checked(input, at);
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

    fn dispatch_checked(&mut self, input: &Input, at: i64) -> HostedResult<HostedReply> {
        let path = path(input);
        match (input.method.as_str(), path) {
            ("GET", "/health") => return Ok(Reply::json(200, &json!({"ok":true})).into()),
            ("GET", "/login") => {
                return Ok(Reply::text(
                    "text/html; charset=utf-8",
                    include_str!("../../ui/login.html"),
                )
                .into());
            }
            ("GET", "/login.js") => {
                return Ok(Reply::text(
                    "text/javascript; charset=utf-8",
                    include_str!("../../ui/login.js"),
                )
                .into());
            }
            ("GET", "/login.css") => {
                return Ok(Reply::text(
                    "text/css; charset=utf-8",
                    include_str!("../../ui/login.css"),
                )
                .into());
            }
            ("POST", "/auth/login") => return self.login(input, at),
            _ => {}
        }
        let token = self.token(input);
        let identity = match token {
            Some(token) => self.store.authenticate(token, at)?,
            None => None,
        };
        let Some(identity) = identity else {
            if path == "/" {
                let mut reply = error(302, "ログインしてください。");
                reply.headers.push(("Location", "/login".into()));
                return Ok(reply);
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
            self.store.logout(token.ok_or("sessionがありません。")?)?;
            self.store.audit(
                &identity.username,
                Some(&identity.tenant),
                "logout",
                "success",
            )?;
            let mut reply: HostedReply = Reply::json(200, &json!({"ok":true})).into();
            reply
                .headers
                .push(("Set-Cookie", self.config.cookie("", 0)));
            reply
                .headers
                .push(("Clear-Site-Data", "\"cache\", \"storage\"".into()));
            return Ok(reply);
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
        if input.method == "POST"
            && ((identity.role == Role::Viewer && !read_only_post(input))
                || (path.ends_with("/erase") && identity.role != Role::Admin))
        {
            return self.denied(&identity);
        }
        if path.starts_with("/auth/") {
            return Ok(error(404, "対象が見つかりません。"));
        }
        self.dispatch_crm(input, &identity)
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
        let mut reply = crate::server::dispatch(self.engine(&identity.tenant)?, &trusted, 0);
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
            reply.body = html
                .replace(
                    "<script src=",
                    "<script type=\"application/x-meibo-gated\" data-meibo-src=",
                )
                .replace("<main ", "<main hidden data-meibo-gated ")
                .replace(
                    "</head>",
                    "<link rel=\"stylesheet\" href=\"/login.css\"></head>",
                )
                .replace(
                    "</body>",
                    "<script src=\"/login.js\" defer></script></body>",
                )
                .into_bytes();
        }
        Ok(reply.into())
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

    fn login(&mut self, input: &Input, at: i64) -> HostedResult<HostedReply> {
        let value: Value = match serde_json::from_slice(&input.body) {
            Ok(v) => v,
            Err(_) => return Ok(error(400, "入力内容を確認してください。")),
        };
        let Some((tenant, name, password)) = value
            .get("tenant")
            .and_then(Value::as_str)
            .zip(value.get("username").and_then(Value::as_str))
            .zip(value.get("password").and_then(Value::as_str))
            .map(|((t, n), p)| (t, n, p))
        else {
            return Ok(error(400, "入力内容を確認してください。"));
        };
        match self.store.login(tenant, name, password, at)? {
            LoginOutcome::Invalid => Ok(error(401, "組織・利用者名・passwordを確認してください。")),
            LoginOutcome::Throttled => Ok(error(
                429,
                "ログイン試行の上限です。15分後にもう一度操作してください。",
            )),
            LoginOutcome::Success(login) => {
                let mut reply: HostedReply =
                    Reply::json(200, &identity_json(&login.identity)).into();
                reply
                    .headers
                    .push(("Set-Cookie", self.config.cookie(&login.token, 8 * 3600)));
                Ok(reply)
            }
        }
    }

    fn engine(&mut self, tenant: &str) -> HostedResult<&mut Engine> {
        if let Some(pos) = self.engines.iter().position(|(slug, _)| slug == tenant) {
            let entry = self.engines.remove(pos).ok_or("接続を取得できません。")?;
            self.engines.push_back(entry);
        } else {
            let opened = self.store.open_engine(tenant);
            #[cfg(feature = "cloud")]
            if self.store.remote()
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
        let server =
            Server::http(self.config.bind).map_err(|_| "hostedサーバーを起動できません。")?;
        println!(
            "hosted listening: {} (public origin: {})",
            server.server_addr(),
            self.config.origin
        );
        std::io::stdout().flush()?;
        for mut request in server.incoming_requests() {
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
            let reply = if let Some(reply) = self.boundary(&input) {
                reply
            } else if request
                .body_length()
                .is_some_and(|size| size > limit(&input))
            {
                error(413, "送信内容が上限を超えています。")
            } else {
                if input.method == "POST"
                    && request
                        .as_reader()
                        .take((limit(&input) + 1) as u64)
                        .read_to_end(&mut input.body)
                        .is_err()
                {
                    send(request, error(400, "送信内容を読み取れません。"));
                    continue;
                }
                self.dispatch(&input, now()?)
            };
            send(request, reply);
        }
        Ok(())
    }
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
        ("Cache-Control", "no-store"),
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
