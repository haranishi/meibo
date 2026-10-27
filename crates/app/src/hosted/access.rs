//! Explicit Cloudflare Access identity, verified before the storage boundary.
mod keys;

use super::store;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
#[cfg(test)]
pub(super) use keys::KeySource;
use keys::{KeyCache, NetworkSource};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    io::Read,
    path::Path,
};

const MAX_CONFIG: usize = 32 * 1024;
const MAX_TOKEN: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AccessError {
    Configuration,
    Invalid,
    Unavailable,
}
impl AccessError {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::Configuration => "Cloudflareログインの設定を確認してください。",
            Self::Invalid => "Cloudflareでログインし直してください。",
            Self::Unavailable => "Cloudflareのログイン状態を確認できません。",
        }
    }
}
impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for AccessError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputConfig {
    version: u8,
    issuer: String,
    audience: String,
    users: Vec<Binding>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    email: String,
    username: String,
}

pub(super) struct Config {
    issuer: String,
    audience: String,
    users: BTreeMap<String, String>,
}
impl Config {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, AccessError> {
        let invalid = AccessError::Configuration;
        if bytes.len() > MAX_CONFIG {
            return Err(invalid);
        }
        let input: InputConfig = serde_json::from_slice(bytes).map_err(|_| invalid)?;
        let url = url::Url::parse(&input.issuer).map_err(|_| invalid)?;
        let team = url
            .host_str()
            .and_then(|h| h.strip_suffix(".cloudflareaccess.com"))
            .ok_or(invalid)?;
        if input.version != 1
            || url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || input.issuer != format!("https://{}", url.host_str().ok_or(invalid)?)
            || team.is_empty()
            || team.len() > 63
            || team.starts_with('-')
            || team.ends_with('-')
            || !team
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || input.audience.is_empty()
            || input.audience.len() > 256
            || !input.audience.bytes().all(|b| b.is_ascii_graphic())
            || input.users.is_empty()
            || input.users.len() > 64
        {
            return Err(invalid);
        }
        let mut users = BTreeMap::new();
        let mut names = HashSet::new();
        for binding in input.users {
            let email = email(&binding.email).ok_or(invalid)?;
            let name = store::username(&binding.username).map_err(|_| invalid)?;
            if users.insert(email, name.clone()).is_some() || !names.insert(name) {
                return Err(invalid);
            }
        }
        Ok(Self {
            issuer: input.issuer,
            audience: input.audience,
            users,
        })
    }
    pub(super) fn load(path: &Path) -> Result<Self, AccessError> {
        let invalid = AccessError::Configuration;
        let meta = path.symlink_metadata().map_err(|_| invalid)?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_CONFIG as u64 {
            return Err(invalid);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(invalid);
            }
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| invalid)?
            .take((MAX_CONFIG + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid)?;
        Self::parse(&bytes)
    }
    pub(super) fn certs_url(&self) -> String {
        format!("{}/cdn-cgi/access/certs", self.issuer)
    }
}

fn email(value: &str) -> Option<String> {
    let (local, domain) = value.split_once('@')?;
    if value.len() > 254
        || local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

#[derive(Deserialize)]
struct Claims {
    exp: i64,
    iat: i64,
    #[serde(default, deserialize_with = "optional_time")]
    nbf: Option<i64>,
    sub: String,
    email: String,
    #[serde(rename = "iss")]
    _issuer: String,
    #[serde(rename = "aud")]
    _audience: Vec<String>,
    #[serde(rename = "type")]
    kind: String,
}
fn optional_time<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<i64>, D::Error> {
    i64::deserialize(value).map(Some)
}

pub(super) struct Access {
    config: Config,
    cache: KeyCache,
    source: Box<dyn keys::KeySource>,
}
impl Access {
    pub(super) fn new(config: Config) -> Result<Self, AccessError> {
        let source = Box::new(NetworkSource::new(config.certs_url())?);
        Ok(Self {
            config,
            cache: KeyCache::default(),
            source,
        })
    }
    #[cfg(test)]
    pub(super) fn with_source(config: Config, source: Box<dyn KeySource>) -> Self {
        Self {
            config,
            cache: KeyCache::default(),
            source,
        }
    }
    pub(super) fn verify(&mut self, token: &str, at: i64) -> Result<String, AccessError> {
        let invalid = AccessError::Invalid;
        if token.is_empty() || token.len() > MAX_TOKEN || at < 0 {
            return Err(invalid);
        }
        let header = decode_header(token).map_err(|_| invalid)?;
        if header.alg != Algorithm::RS256 || header.typ.as_deref().is_some_and(|v| v != "JWT") {
            return Err(invalid);
        }
        let raw = URL_SAFE_NO_PAD
            .decode(token.split('.').next().ok_or(invalid)?)
            .map_err(|_| invalid)?;
        let raw: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| invalid)?;
        if raw.get("crit").is_some() {
            return Err(invalid);
        }
        let kid = header.kid.filter(|k| keys::valid_kid(k)).ok_or(invalid)?;
        let key = self.cache.get(&kid, at, self.source.as_mut())?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        // The HTTP clock is shared with session expiry and injected by tests.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.leeway = 0;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| invalid)?
            .claims;
        if claims.exp <= at
            || claims.iat < 0
            || claims.iat > at
            || claims.exp <= claims.iat
            || claims.nbf.is_some_and(|v| v < 0 || v > at)
            || claims.sub.is_empty()
            || claims.sub.len() > 256
            || !claims.sub.bytes().all(|b| b.is_ascii_graphic())
            || claims.kind != "app"
        {
            return Err(invalid);
        }
        let email = email(&claims.email).ok_or(invalid)?;
        self.config.users.get(&email).cloned().ok_or(invalid)
    }
}
