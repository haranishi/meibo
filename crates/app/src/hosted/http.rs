use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::SocketAddr,
};

use meibo_core::Engine;
use serde_json::{Value, json};
use tiny_http::{Header, Request, Response, Server, StatusCode};

use super::{HostedResult, Identity, LoginOutcome, Role, Store, now, store::engine_options};
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

pub(super) struct Hosted {
    pub(super) store: Store,
    config: HostedConfig,
    engines: VecDeque<(String, Engine)>,
}
impl Hosted {
    pub(super) fn new(store: Store, config: HostedConfig) -> Self {
        Self {
            store,
            config,
            engines: VecDeque::new(),
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
        let Ok(_guard) = self.store.lock() else {
            return error(503, "保守処理中です。しばらくしてから操作してください。");
        };
        match self.dispatch_checked(input, at) {
            Ok(reply) => reply,
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
            let path = self.store.tenant_path(tenant)?;
            if self.engines.len() >= 4 {
                self.engines.pop_front();
            }
            self.engines
                .push_back((tenant.into(), Engine::open(&path, &engine_options())?));
        }
        self.engines
            .back_mut()
            .map(|(_, engine)| engine)
            .ok_or_else(|| "接続を取得できません。".into())
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
