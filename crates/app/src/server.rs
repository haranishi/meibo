//! 同梱画面から合成デモを操作する、loopback専用のHTTP入口。
mod api;

use std::{
    error::Error,
    io::{Read, Write},
    path::Path,
};

use meibo_core::{Engine, OpenOptions, types::DataOrigin};
use serde_json::{Value, json};
use tiny_http::{Header, Request, Response, Server, StatusCode};

const MAX_BODY: usize = 1024 * 1024;
const MAX_EXPORT: usize = 32 * 1024 * 1024;
const SECURITY_HEADERS: [(&str, &str); 5] = [
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Cache-Control", "no-store"),
    ("Referrer-Policy", "no-referrer"),
];

pub(crate) fn serve(db: &Path, port: u16) -> Result<(), Box<dyn Error>> {
    let mut engine = open_demo(db)?;
    let server = Server::http(("127.0.0.1", port))
        .map_err(|_| "ローカルサーバーを起動できません。ポートの使用状況を確認してください。")?;
    let address = server
        .server_addr()
        .to_ip()
        .ok_or("ローカルアドレスを取得できません。")?;
    let port = address.port();
    println!("http://127.0.0.1:{port}");
    std::io::stdout().flush()?;
    println!("停止: Ctrl+C（営業メモはDBに保存されます）");
    for request in server.incoming_requests() {
        handle_http(&mut engine, request, port);
    }
    Ok(())
}

fn open_demo(db: &Path) -> Result<Engine, Box<dyn Error>> {
    let engine = Engine::open(db, &OpenOptions::default())
        .map_err(|_| "既存のデモDBを開けません。パスと保存形式を確認してください。")?;
    if engine.origin() != DataOrigin::Synthetic {
        return Err("ローカル画面は合成デモDBだけで起動できます。".into());
    }
    Ok(engine)
}

#[derive(Debug)]
struct Input {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Input {
    fn single_header(&self, name: &str) -> Option<&str> {
        let mut values = self
            .headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name));
        let (_, value) = values.next()?;
        if values.next().is_some() {
            return None;
        }
        Some(value)
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
    attachment: bool,
}

impl Reply {
    fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            mime: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
            attachment: false,
        }
    }

    fn error(status: u16, message: &'static str) -> Self {
        Self::json(status, &json!({"error": message}))
    }

    fn text(mime: &'static str, body: &'static str) -> Self {
        Self {
            status: 200,
            mime,
            body: body.as_bytes().to_vec(),
            attachment: false,
        }
    }
}

fn handle_http(engine: &mut Engine, mut request: Request, port: u16) {
    let mut input = Input {
        method: request.method().as_str().to_owned(),
        target: request.url().to_owned(),
        headers: request
            .headers()
            .iter()
            .map(|header| (header.field.as_str().to_string(), header.value.to_string()))
            .collect(),
        body: Vec::new(),
    };
    let reply = if let Err(reply) = validate_boundary(&input, port) {
        reply
    } else {
        if input.method == "POST" {
            if request.body_length().is_some_and(|size| size > MAX_BODY) {
                send_reply(request, Reply::error(413, "送信内容が上限を超えています。"));
                return;
            }
            let result = request
                .as_reader()
                .take((MAX_BODY + 1) as u64)
                .read_to_end(&mut input.body);
            if result.is_err() {
                send_reply(request, Reply::error(400, "送信内容を読み取れません。"));
                return;
            }
        }
        dispatch(engine, &input, port)
    };
    send_reply(request, reply);
}

fn send_reply(request: Request, reply: Reply) {
    let mut response = Response::from_data(reply.body).with_status_code(StatusCode(reply.status));
    for (name, value) in SECURITY_HEADERS
        .into_iter()
        .chain(std::iter::once(("Content-Type", reply.mime)))
    {
        if let Ok(header) = Header::from_bytes(name, value) {
            response.add_header(header);
        }
    }
    if reply.attachment {
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

fn validate_boundary(input: &Input, port: u16) -> Result<(), Reply> {
    let Some(host) = input.single_header("Host") else {
        return Err(Reply::error(403, "ローカルURLからアクセスしてください。"));
    };
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err(Reply::error(403, "ローカルURLからアクセスしてください。"));
    }
    // HTTPのabsolute-formやURLを使ったパス解釈のずれを受け付けない。
    if !input.target.starts_with('/') || input.target.starts_with("//") || input.target.len() > 8192
    {
        return Err(Reply::error(400, "URLが正しくありません。"));
    }
    if input.method == "POST" {
        if input.single_header("Origin") != Some(format!("http://{host}").as_str())
            || input.single_header("X-Meibo-Local") != Some("1")
        {
            return Err(Reply::error(403, "この画面から保存してください。"));
        }
        let content_type = input
            .single_header("Content-Type")
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if !content_type.is_some_and(|value| value.eq_ignore_ascii_case("application/json")) {
            return Err(Reply::error(415, "JSON形式で送信してください。"));
        }
    }
    Ok(())
}

fn dispatch(engine: &mut Engine, input: &Input, port: u16) -> Reply {
    if let Err(reply) = validate_boundary(input, port) {
        return reply;
    }
    if input.body.len() > MAX_BODY {
        return Reply::error(413, "送信内容が上限を超えています。");
    }
    if input.method != "GET" && input.method != "POST" {
        return Reply::error(405, "この操作には対応していません。");
    }
    let (path, query) = input.target.split_once('?').unwrap_or((&input.target, ""));
    if input.method == "POST" && path == "/api/companies" {
        return api::create_company(engine, &input.body);
    }
    if input.method == "GET" {
        match path {
            "/" => {
                return Reply::text("text/html; charset=utf-8", include_str!("../ui/index.html"));
            }
            "/style.css" => {
                return Reply::text("text/css; charset=utf-8", include_str!("../ui/style.css"));
            }
            "/app.js" => {
                return Reply::text(
                    "text/javascript; charset=utf-8",
                    include_str!("../ui/app.js"),
                );
            }
            "/health" => return Reply::json(200, &json!({"ok":true})),
            "/api/summary" => return api::summary(engine),
            "/api/companies" => return api::companies(engine, query),
            "/api/export" => return api::export(engine),
            _ => {}
        }
    }
    if let Some(rest) = path.strip_prefix("/api/companies/") {
        if input.method == "POST" {
            if let Some(id) = rest.strip_suffix("/activities").and_then(company_id) {
                return api::activity(engine, id, &input.body);
            }
        } else if let Some(id) = company_id(rest) {
            return api::detail(engine, id);
        }
    }
    Reply::error(404, "対象が見つかりません。")
}

fn company_id(value: &str) -> Option<meibo_core::types::CompanyId> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(meibo_core::types::CompanyId)
}

#[cfg(test)]
mod tests;
