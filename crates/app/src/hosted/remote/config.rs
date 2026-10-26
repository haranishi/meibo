//! Bounded, reference-only remote configuration.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[cfg(feature = "integration-tests")]
use std::net::IpAddr;

use meibo_db::RemoteOptions;
use serde::{Deserialize, Deserializer, de};
use url::Url;

use super::super::{HostedResult, store::valid_slug};

const MAX_CONFIG: usize = 1024 * 1024;
pub(super) const MAX_TENANTS: usize = 1000;
const CONFIG_ERROR: &str = "リモート保存設定を確認してください。";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::hosted) struct RemoteConfig {
    version: u32,
    control: References,
    tenants: TenantReferences,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct References {
    url_env: String,
    token_env: String,
}

struct TenantReferences(BTreeMap<String, References>);

impl<'de> Deserialize<'de> for TenantReferences {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = TenantReferences;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("bounded unique tenant references")
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut tenants = BTreeMap::new();
                while let Some(slug) = map.next_key::<String>()? {
                    if tenants.len() >= MAX_TENANTS
                        || !valid_slug(&slug)
                        || tenants.contains_key(&slug)
                    {
                        return Err(de::Error::custom(CONFIG_ERROR));
                    }
                    tenants.insert(slug, map.next_value()?);
                }
                Ok(TenantReferences(tenants))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Clone)]
pub(in crate::hosted) struct Endpoints {
    pub(super) control: RemoteOptions,
    pub(super) tenants: BTreeMap<String, RemoteOptions>,
}

#[derive(Clone, Copy)]
pub(in crate::hosted) enum EndpointPolicy {
    Production,
    #[cfg(feature = "integration-tests")]
    LoopbackTests,
}

impl RemoteConfig {
    pub(in crate::hosted) fn parse(bytes: &[u8]) -> HostedResult<Self> {
        if bytes.len() > MAX_CONFIG {
            return Err(CONFIG_ERROR.into());
        }
        let config: Self = serde_json::from_slice(bytes).map_err(|_| CONFIG_ERROR)?;
        if config.version != 1 {
            return Err(CONFIG_ERROR.into());
        }
        let mut seen = BTreeSet::new();
        for reference in std::iter::once(&config.control).chain(config.tenants.0.values()) {
            for name in [&reference.url_env, &reference.token_env] {
                if !valid_reference(name) || !seen.insert(name.as_str()) {
                    return Err(CONFIG_ERROR.into());
                }
            }
        }
        Ok(config)
    }

    pub(in crate::hosted) fn resolve(&self) -> HostedResult<Endpoints> {
        self.resolve_with(|name| std::env::var(name).ok(), EndpointPolicy::Production)
    }

    pub(in crate::hosted) fn resolve_with(
        &self,
        mut lookup: impl FnMut(&str) -> Option<String>,
        policy: EndpointPolicy,
    ) -> HostedResult<Endpoints> {
        let mut seen = BTreeSet::new();
        let mut resolve = |reference: &References| -> HostedResult<RemoteOptions> {
            let endpoint = lookup(&reference.url_env).ok_or(CONFIG_ERROR)?;
            let token = lookup(&reference.token_env).ok_or(CONFIG_ERROR)?;
            let endpoint = normalized_endpoint(&endpoint, policy).ok_or(CONFIG_ERROR)?;
            if token.is_empty()
                || token.len() > 8192
                || !token.bytes().all(|byte| byte.is_ascii_graphic())
                || !seen.insert(endpoint.clone())
            {
                return Err(CONFIG_ERROR.into());
            }
            let options = RemoteOptions::new(endpoint, token);
            #[cfg(feature = "integration-tests")]
            let options = if matches!(policy, EndpointPolicy::LoopbackTests) {
                options.allow_loopback_http_for_tests()
            } else {
                options
            };
            Ok(options)
        };
        let control = resolve(&self.control)?;
        let tenants = self
            .tenants
            .0
            .iter()
            .map(|(slug, references)| Ok((slug.clone(), resolve(references)?)))
            .collect::<HostedResult<_>>()?;
        Ok(Endpoints { control, tenants })
    }
}

fn valid_reference(value: &str) -> bool {
    value.strip_prefix("MEIBO_DB_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

fn normalized_endpoint(raw: &str, policy: EndpointPolicy) -> Option<String> {
    let (_, authority) = raw.split_once("://")?;
    let authority = authority.split(['/', '?', '#']).next()?;
    if authority.is_empty()
        || authority.contains('@')
        || raw.contains('\\')
        || raw
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return None;
    }
    let mut url = Url::parse(raw).ok()?;
    if url.scheme() == "libsql" {
        url = Url::parse(&format!("https:{}", &raw[7..])).ok()?;
    }
    let loopback = permits_loopback(policy, &url);
    if (url.scheme() != "https" && !loopback)
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    Some(url.to_string())
}

fn permits_loopback(policy: EndpointPolicy, url: &Url) -> bool {
    #[cfg(feature = "integration-tests")]
    if matches!(policy, EndpointPolicy::LoopbackTests) && url.scheme() == "http" {
        return url
            .host_str()
            .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
    }
    let _ = (policy, url);
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn config() -> String {
        json!({"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"},"beta":{"url_env":"MEIBO_DB_BETA_URL","token_env":"MEIBO_DB_BETA_TOKEN"}}}).to_string()
    }

    fn lookup(name: &str) -> Option<String> {
        if name.ends_with("TOKEN") {
            Some("synthetic-fixture-token".into())
        } else if name.ends_with("URL") {
            Some(format!(
                "https://{}.example.test",
                name.to_ascii_lowercase()
            ))
        } else {
            None
        }
    }

    #[test]
    fn valid_reference_only_zero_tenant_configuration_is_accepted() {
        let bytes = br#"{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{}}"#;
        let endpoints = RemoteConfig::parse(bytes)
            .unwrap()
            .resolve_with(lookup, EndpointPolicy::Production)
            .unwrap();
        assert!(endpoints.tenants.is_empty());
    }

    #[test]
    fn valid_two_tenant_configuration_is_accepted() {
        let parsed = RemoteConfig::parse(config().as_bytes()).unwrap();
        assert_eq!(
            parsed
                .resolve_with(lookup, EndpointPolicy::Production)
                .unwrap()
                .tenants
                .len(),
            2
        );
    }

    #[test]
    fn unknown_duplicate_and_invalid_fields_have_fixed_diagnostics() {
        let base = config();
        let cases = [
            base.replace("\"version\":1", "\"version\":2"),
            base.replace("\"version\":1", "\"version\":1,\"version\":1"),
            base.replace("\"version\":1", "\"version\":1,\"private-secret-field\":true"),
            base.replace("\"url_env\":\"MEIBO_DB_CONTROL_URL\"", "\"url_env\":\"MEIBO_DB_CONTROL_URL\",\"url_env\":\"MEIBO_DB_CONTROL_URL\""),
            base.replace("\"url_env\":\"MEIBO_DB_CONTROL_URL\"", "\"url_env\":\"MEIBO_DB_CONTROL_URL\",\"bad-secret-key\":\"secret-value\""),
            base.replace("\"alpha\":", "\"../alpha\":"),
            base.replace("MEIBO_DB_ALPHA_URL", "MEIBO_DB_CONTROL_URL"),
            base.replace("MEIBO_DB_ALPHA_URL", "OTHER_URL"),
            base.replace("MEIBO_DB_ALPHA_URL", "MEIBO_DB_lowercase"),
            base.replace("MEIBO_DB_ALPHA_URL", "MEIBO_DB_"),
            base.replace("\"alpha\":", "\"alpha\":{\"url_env\":\"MEIBO_DB_OTHER_URL\",\"token_env\":\"MEIBO_DB_OTHER_TOKEN\"},\"alpha\":"),
            "{\"secret-json\":\"unterminated".into(),
        ];
        for bytes in cases {
            let error = RemoteConfig::parse(bytes.as_bytes())
                .err()
                .expect("reject invalid config");
            assert_eq!(error.to_string(), CONFIG_ERROR);
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn tenant_count_and_raw_input_are_bounded() {
        let mut value = json!({"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{}});
        for number in 0..MAX_TENANTS {
            value["tenants"][format!("t{number}")] = json!({"url_env":format!("MEIBO_DB_T{number}_URL"),"token_env":format!("MEIBO_DB_T{number}_TOKEN")});
        }
        assert!(RemoteConfig::parse(value.to_string().as_bytes()).is_ok());
        value["tenants"]["over-limit"] =
            json!({"url_env":"MEIBO_DB_OVER_URL","token_env":"MEIBO_DB_OVER_TOKEN"});
        assert!(RemoteConfig::parse(value.to_string().as_bytes()).is_err());
        let mut padded = config();
        padded.push_str(&" ".repeat(MAX_CONFIG - padded.len()));
        assert!(RemoteConfig::parse(padded.as_bytes()).is_ok());
        padded.push(' ');
        assert!(RemoteConfig::parse(padded.as_bytes()).is_err());
    }

    #[test]
    fn missing_empty_and_duplicate_resolved_secrets_are_rejected_without_echo() {
        let config = RemoteConfig::parse(config().as_bytes()).unwrap();
        for replacement in [None, Some(String::new()), Some("secret\ncredential".into())] {
            let error = config
                .resolve_with(
                    |name| {
                        if name == "MEIBO_DB_ALPHA_TOKEN" {
                            replacement.clone()
                        } else {
                            lookup(name)
                        }
                    },
                    EndpointPolicy::Production,
                )
                .err()
                .unwrap();
            assert_eq!(error.to_string(), CONFIG_ERROR);
        }
        for endpoint in [
            "https://CONTROL.example.test:443/",
            "libsql://control.example.test",
        ] {
            let error = config
                .resolve_with(
                    |name| {
                        if name == "MEIBO_DB_CONTROL_URL" {
                            Some("https://control.example.test".into())
                        } else if name == "MEIBO_DB_ALPHA_URL" {
                            Some(endpoint.into())
                        } else {
                            lookup(name)
                        }
                    },
                    EndpointPolicy::Production,
                )
                .err()
                .unwrap();
            assert_eq!(error.to_string(), CONFIG_ERROR);
        }
    }

    #[test]
    fn endpoint_validation_matches_production_transport_policy() {
        for raw in [
            "http://127.0.0.1:8080",
            "https://a.example.test/path",
            "https://a.example.test?secret",
            "https://user:secret@a.example.test",
            "https://@a.example.test",
            "https://a.example.test/#secret",
            "https://a.example.test\\repair",
            " https://a.example.test",
        ] {
            assert!(normalized_endpoint(raw, EndpointPolicy::Production).is_none());
        }
        assert_eq!(
            normalized_endpoint("libsql://EXAMPLE.test:443", EndpointPolicy::Production).as_deref(),
            Some("https://example.test/")
        );
    }

    #[cfg(feature = "integration-tests")]
    #[test]
    fn test_policy_allows_only_literal_loopback_http() {
        assert!(
            normalized_endpoint("http://127.0.0.1:8080", EndpointPolicy::LoopbackTests).is_some()
        );
        for raw in [
            "http://localhost:8080",
            "http://192.0.2.1",
            "http://127.0.0.1:8080/secret",
        ] {
            assert!(normalized_endpoint(raw, EndpointPolicy::LoopbackTests).is_none());
        }
    }
}
