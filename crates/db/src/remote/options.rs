use std::{fmt, net::IpAddr, time::Duration};

use reqwest::{Url, header::HeaderValue};

use crate::{Result, remote::failure};

const MAX_TIMEOUT: Duration = Duration::from_secs(120);
pub(super) const MAX_BODY: usize = 32 * 1024 * 1024;

/// Private connection credentials and finite request/response limits.
#[derive(Clone)]
pub struct RemoteOptions {
    endpoint: String,
    token: String,
    timeout: Duration,
    response_limit: usize,
    allow_loopback_http: bool,
}

impl RemoteOptions {
    /// Configure a production HTTPS/libsql endpoint and bearer token.
    /// Validation occurs when constructing the remote connection.
    #[must_use]
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            token: token.into(),
            timeout: Duration::from_secs(30),
            response_limit: 16 * 1024 * 1024,
            allow_loopback_http: false,
        }
    }

    /// Permit HTTP only to a literal loopback IP for synthetic local tests.
    #[must_use]
    pub const fn allow_loopback_http_for_tests(mut self) -> Self {
        self.allow_loopback_http = true;
        self
    }

    /// Set the entire request's deadline, including response body reception.
    ///
    /// # Errors
    /// Rejects zero or a duration greater than 120 seconds.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > MAX_TIMEOUT {
            return Err(failure("invalid remote database limits"));
        }
        self.timeout = timeout;
        Ok(self)
    }

    /// Set the maximum received bytes per success or error response.
    ///
    /// # Errors
    /// Rejects zero or a limit greater than 32 MiB.
    pub fn with_response_limit(mut self, limit: usize) -> Result<Self> {
        if limit == 0 || limit > MAX_BODY {
            return Err(failure("invalid remote database limits"));
        }
        self.response_limit = limit;
        Ok(self)
    }

    /// Return the configured whole-request deadline.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Return the configured maximum bytes in one response body.
    #[must_use]
    pub const fn response_limit(&self) -> usize {
        self.response_limit
    }

    pub(super) fn validated(self) -> Result<(Url, HeaderValue)> {
        // Reject URL parser repairs of whitespace/backslashes and noncanonical
        // authority forms instead of silently changing a configured endpoint.
        if !safe_raw_url(&self.endpoint) {
            return Err(failure("invalid remote database configuration"));
        }
        let mut url = Url::parse(&self.endpoint)
            .map_err(|_| failure("invalid remote database configuration"))?;
        if url.scheme() == "libsql" {
            // libsql is a non-special URL scheme; reparsing as HTTPS performs
            // the normal authority/IDNA and port validation too.
            url = Url::parse(&format!("https:{}", &self.endpoint[7..]))
                .map_err(|_| failure("invalid remote database configuration"))?;
        }
        let http_loopback =
            self.allow_loopback_http && url.scheme() == "http" && literal_loopback(&url);
        if (url.scheme() != "https" && !http_loopback)
            || !safe_url(&url)
            || url.path() != "/"
            || self.token.is_empty()
            || self.token.len() > 8192
            || !self.token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(failure("invalid remote database configuration"));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.token))
            .map_err(|_| failure("invalid remote database configuration"))?;
        authorization.set_sensitive(true);
        Ok((url, authorization))
    }
}

pub(super) fn safe_url(url: &Url) -> bool {
    url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

pub(super) fn safe_raw_url(url: &str) -> bool {
    let Some((_, authority)) = url.split_once("://") else {
        return false;
    };
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !url.contains('\\')
        && !url
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}

fn literal_loopback(url: &Url) -> bool {
    url.host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
}

impl fmt::Debug for RemoteOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteOptions")
            .field("credentials", &"<REDACTED>")
            .field("timeout", &self.timeout)
            .field("response_limit", &self.response_limit)
            .field("allow_loopback_http", &self.allow_loopback_http)
            .finish_non_exhaustive()
    }
}
