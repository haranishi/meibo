use super::AccessError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{DecodingKey, jwk::Jwk};
use std::collections::HashMap;

const MAX_KEYS: usize = 16;
const MAX_BYTES: usize = 64 * 1024;
pub(in crate::hosted) trait KeySource {
    fn fetch(&mut self) -> Result<Vec<u8>, AccessError>;
}

pub(super) struct NetworkSource {
    url: String,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}
impl NetworkSource {
    pub(super) fn new(url: String) -> Result<Self, AccessError> {
        let unavailable = AccessError::Unavailable;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| unavailable)?;
        let client = reqwest::Client::builder()
            .http1_only()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(10))
            .tls_version_min(reqwest::tls::Version::TLS_1_2)
            .build()
            .map_err(|_| unavailable)?;
        Ok(Self {
            url,
            client,
            runtime,
        })
    }
}
impl KeySource for NetworkSource {
    fn fetch(&mut self) -> Result<Vec<u8>, AccessError> {
        self.runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let fail = AccessError::Unavailable;
                let mut response = self.client.get(&self.url).send().await.map_err(|_| fail)?;
                if !response.status().is_success()
                    || response
                        .content_length()
                        .is_some_and(|n| n > MAX_BYTES as u64)
                {
                    return Err(fail);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| fail)? {
                    if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
                        return Err(fail);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(bytes)
            })
            .await
            .map_err(|_| AccessError::Unavailable)?
        })
    }
}

pub(super) fn valid_kid(kid: &str) -> bool {
    !kid.is_empty()
        && kid.len() <= 128
        && kid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn parse(bytes: &[u8]) -> Result<HashMap<String, DecodingKey>, AccessError> {
    let fail = AccessError::Unavailable;
    if bytes.len() > MAX_BYTES {
        return Err(fail);
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| fail)?;
    let keys = value
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .ok_or(fail)?;
    if keys.is_empty() || keys.len() > MAX_KEYS {
        return Err(fail);
    }
    let mut result = HashMap::new();
    for key in keys {
        if key.get("kty").and_then(serde_json::Value::as_str) != Some("RSA") {
            continue;
        }
        let text = |name| key.get(name).and_then(serde_json::Value::as_str);
        let kid = text("kid").filter(|v| valid_kid(v)).ok_or(fail)?;
        if text("alg").is_some_and(|v| v != "RS256")
            || text("use").is_some_and(|v| v != "sig")
            || ["d", "p", "q", "dp", "dq", "qi", "oth"]
                .iter()
                .any(|v| key.get(v).is_some())
        {
            return Err(fail);
        }
        if key.get("key_ops").is_some_and(|v| {
            !v.as_array()
                .is_some_and(|v| v.iter().any(|x| x.as_str() == Some("verify")))
        }) {
            return Err(fail);
        }
        let modulus = URL_SAFE_NO_PAD
            .decode(text("n").ok_or(fail)?)
            .map_err(|_| fail)?;
        let exponent = URL_SAFE_NO_PAD
            .decode(text("e").ok_or(fail)?)
            .map_err(|_| fail)?;
        if !(256..=1024).contains(&modulus.len())
            || modulus.first() == Some(&0)
            || exponent.is_empty()
            || exponent.len() > 8
        {
            return Err(fail);
        }
        let jwk: Jwk = serde_json::from_value(key.clone()).map_err(|_| fail)?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| fail)?;
        if result.insert(kid.to_owned(), key).is_some() {
            return Err(fail);
        }
    }
    if result.is_empty() {
        return Err(fail);
    }
    Ok(result)
}

#[derive(Default)]
pub(super) struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    loaded_at: Option<i64>,
    next_refresh: i64,
}
impl KeyCache {
    pub(super) fn get(
        &mut self,
        kid: &str,
        at: i64,
        source: &mut dyn KeySource,
    ) -> Result<DecodingKey, AccessError> {
        let fresh = self
            .loaded_at
            .is_some_and(|loaded| at >= loaded && at < loaded.saturating_add(300));
        if fresh && let Some(key) = self.keys.get(kid) {
            return Ok(key.clone());
        }
        if at < self.next_refresh {
            return Err(if fresh {
                AccessError::Invalid
            } else {
                AccessError::Unavailable
            });
        }
        self.next_refresh = at.saturating_add(30);
        self.keys = parse(&source.fetch()?)?;
        self.loaded_at = Some(at);
        self.keys.get(kid).cloned().ok_or(AccessError::Invalid)
    }
}
