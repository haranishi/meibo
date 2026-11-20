//! Opt-in, server-side OAuth. Only sanitized CRM records leave the private connector.
#[cfg(test)]
mod tests;
mod vault;
use super::{Input, Reply};
use meibo_core::{Engine, types::DataOrigin};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const WORKER: &str = include_str!("../../connector/providers.cjs");
const PROVIDERS: [&str; 2] = ["google", "microsoft"];
type Result<T> = std::result::Result<T, Failure>;

#[derive(Clone, Copy, Debug)]
enum Failure {
    Input,
    Setup,
    Unavailable,
    Consent,
    Stale,
    Storage,
}
impl Failure {
    fn reply(self) -> Reply {
        let (status, code, message) = match self {
            Self::Input => (400, "input", "入力を確認してください。"),
            Self::Setup => (
                409,
                "setup",
                "管理者が実データ用の接続設定を済ませてから利用できます。",
            ),
            Self::Consent => (
                409,
                "reauth",
                "接続し直してメールと予定の読み取りを許可してください。",
            ),
            Self::Stale => (
                409,
                "stale",
                "接続手続きの期限が切れたか、接続が変わりました。もう一度開いてください。",
            ),
            Self::Unavailable => (
                503,
                "provider",
                "接続先から取得できませんでした。時間を置いて再試行してください。",
            ),
            Self::Storage => (
                503,
                "storage",
                "接続または同期の保存に失敗しました。管理者に確認してください。",
            ),
        };
        Reply::json(status, &json!({"error":message,"code":code}))
    }
}
impl From<rusqlite::Error> for Failure {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}
fn value<'a>(json: &'a Value, key: &str) -> Result<&'a str> {
    json.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(Failure::Unavailable)
}

struct Config {
    node: PathBuf,
    directory: PathBuf,
    redirect: String,
    client: String,
    secret: String,
}
impl Config {
    fn for_provider(provider: &str) -> Option<Self> {
        let private = |key: &str| env::var(key).ok().filter(|s| !s.trim().is_empty());
        let origin = private("MEIBO_SYNC_ORIGIN")?;
        let scheme = origin.strip_prefix("https://").or_else(|| {
            origin
                .strip_prefix("http://127.0.0.1:")
                .filter(|p| p.parse::<u16>().is_ok())
        })?;
        if scheme.is_empty()
            || scheme.contains(['/', '?', '#', '@'])
            || origin.chars().any(char::is_whitespace)
        {
            return None;
        }
        let name = provider.to_ascii_uppercase();
        let node = PathBuf::from(private("MEIBO_ACTIVITY_SYNC_NODE")?);
        let directory = PathBuf::from(private("MEIBO_SYNC_VAULT")?);
        if !node.is_absolute() || !directory.is_absolute() {
            return None;
        }
        Some(Self {
            node,
            directory,
            redirect: format!("{origin}/oauth/activity-return"),
            client: private(&format!("MEIBO_SYNC_{name}_CLIENT_ID"))?,
            secret: private(&format!("MEIBO_SYNC_{name}_CLIENT_SECRET"))?,
        })
    }
    fn worker(&self, provider: &str, mut input: Value) -> Result<Value> {
        input["provider"] = json!(provider);
        input["redirect"] = json!(self.redirect);
        input["client_id"] = json!(self.client);
        input["secret"] = json!(self.secret);
        let mut child = Command::new(&self.node)
            .args(["-e", WORKER, "meibo-activity-worker"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Failure::Setup)?;
        let sent = child
            .stdin
            .take()
            .ok_or(Failure::Unavailable)?
            .write_all(input.to_string().as_bytes());
        let output = child.wait_with_output().map_err(|_| Failure::Unavailable)?;
        if sent.is_err() || !output.status.success() || output.stdout.len() > 2 * 1024 * 1024 {
            return Err(Failure::Unavailable);
        }
        let mut answer: Value =
            serde_json::from_slice(&output.stdout).map_err(|_| Failure::Unavailable)?;
        if let Some(code) = answer["error"].as_str() {
            return Err(if matches!(code, "consent" | "reauth") {
                Failure::Consent
            } else {
                Failure::Unavailable
            });
        }
        answer
            .get_mut("result")
            .map(Value::take)
            .ok_or(Failure::Unavailable)
    }
}

fn scope(engine: &mut Engine, actor: Option<&str>, initialize: bool) -> Result<Option<String>> {
    if let Some(scope) = actor {
        return Ok(Some(format!("hosted:{scope}")));
    }
    let path = engine
        .path()
        .and_then(|p| p.canonicalize().ok())
        .ok_or(Failure::Setup)?;
    let path_key = Sha256::digest(path.as_os_str().as_encoded_bytes());
    Ok(engine
        .sync_local_identity(initialize)
        .map_err(|_| Failure::Storage)?
        .map(|id| format!("local:{path_key:x}:{id}")))
}

pub(crate) fn callback(path: &str) -> Option<Reply> {
    match path {
        "/oauth/activity-return" => Some(Reply::text(
            "text/html; charset=utf-8",
            "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>接続の確認</title><main><p id=\"result\">接続を確認しています。</p></main><script src=\"/oauth/activity-return.js\"></script></html>",
        )),
        "/oauth/activity-return.js" => Some(Reply::text(
            "text/javascript; charset=utf-8",
            include_str!("../../ui/activity-return.js"),
        )),
        _ => None,
    }
}

pub(super) fn dispatch(
    engine: &mut Engine,
    input: &Input,
    path: &str,
    query: &str,
    actor: Option<&str>,
) -> Option<Reply> {
    let rest = path.strip_prefix("/api/activity-sync")?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    Some(match route(engine, input, rest, query, actor) {
        Ok(v) => Reply::json(200, &v),
        Err(e) => e.reply(),
    })
}

fn route(
    engine: &mut Engine,
    input: &Input,
    rest: &str,
    query: &str,
    actor: Option<&str>,
) -> Result<Value> {
    if !query.is_empty() {
        return Err(Failure::Input);
    }
    let real = engine.origin() == DataOrigin::Real;
    if input.method == "GET" && rest.is_empty() {
        let mut providers = Vec::new();
        for provider in PROVIDERS {
            let config = Config::for_provider(provider).filter(|_| real);
            let status = if let Some(c) = config {
                let conn = vault::open(&c.directory)?;
                match scope(engine, actor, false)? {
                    Some(owner) => vault::status(&conn, &owner, provider)?,
                    None => json!({"configured":true,"connected":false}),
                }
            } else {
                json!({"configured":false,"connected":false})
            };
            providers.push(json!({"id":provider,"status":status}));
        }
        return Ok(json!({"real":real,"providers":providers}));
    }
    let Some((provider, action)) = rest.trim_start_matches('/').split_once('/') else {
        return Err(Failure::Input);
    };
    if !PROVIDERS.contains(&provider)
        || input.method != "POST"
        || !matches!(action, "begin" | "finish" | "pull" | "disconnect")
    {
        return Err(Failure::Input);
    }
    let body: Value = serde_json::from_slice(&input.body).map_err(|_| Failure::Input)?;
    let fields = body.as_object().ok_or(Failure::Input)?;
    if (action != "finish" && !fields.is_empty())
        || (action == "finish"
            && (fields.len() != 2 || !fields.contains_key("state") || !fields.contains_key("code")))
    {
        return Err(Failure::Input);
    }
    let config = Config::for_provider(provider)
        .filter(|_| real)
        .ok_or(Failure::Setup)?;
    let mut conn = vault::open(&config.directory)?;
    let owner = scope(engine, actor, action == "begin")?.ok_or(Failure::Stale)?;
    vault::apply(&mut conn, engine, &owner, provider, action, &body, |i| {
        config.worker(provider, i)
    })
}
