use libsql_hrana::proto::{
    ExecuteStreamReq, PipelineReqBody, PipelineRespBody, SequenceStreamReq, Stmt, StmtResult,
    StreamRequest, StreamResponse, StreamResult,
};
use std::{io, net::IpAddr, time::Duration};

pub const BODY_LIMIT: usize = 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimal loopback-only protocol driver for fixture tests, not the SQL adapter.
pub struct Client {
    http: reqwest::Client,
    pipeline: reqwest::Url,
    baton: Option<String>,
}

impl Client {
    pub fn connect(endpoint: &str) -> io::Result<Self> {
        let endpoint = reqwest::Url::parse(endpoint)
            .map_err(|_| io::Error::other("invalid fixture endpoint"))?;
        if endpoint.scheme() != "http"
            || !endpoint
                .host_str()
                .and_then(|host| host.parse::<IpAddr>().ok())
                .is_some_and(|address| address.is_loopback())
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
        {
            return Err(io::Error::other("fixture endpoint must be loopback HTTP"));
        }
        Ok(Self {
            http: http_client()?,
            pipeline: endpoint
                .join("v2/pipeline")
                .map_err(|_| io::Error::other("invalid fixture pipeline"))?,
            baton: None,
        })
    }

    pub fn execute(&mut self, sql: &str) -> io::Result<StmtResult> {
        match self.request(StreamRequest::Execute(ExecuteStreamReq {
            stmt: Stmt::new(sql, true),
        }))? {
            StreamResponse::Execute(response) => Ok(response.result),
            _ => Err(io::Error::other("unexpected fixture execute response")),
        }
    }

    pub fn sequence(&mut self, sql: &str) -> io::Result<()> {
        match self.request(StreamRequest::Sequence(SequenceStreamReq {
            sql: Some(sql.to_owned()),
            ..SequenceStreamReq::default()
        }))? {
            StreamResponse::Sequence(_) => Ok(()),
            _ => Err(io::Error::other("unexpected fixture sequence response")),
        }
    }

    fn request(&mut self, request: StreamRequest) -> io::Result<StreamResponse> {
        let body = serde_json::to_vec(&PipelineReqBody {
            baton: self.baton.clone(),
            requests: vec![request],
        })
        .map_err(|_| io::Error::other("cannot encode fixture request"))?;
        if body.len() > BODY_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "fixture request body exceeds limit",
            ));
        }
        let mut response: PipelineRespBody = meibo_db::runtime()?.block_on(async {
            let mut response = self
                .http
                .post(self.pipeline.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|_| io::Error::other("fixture request failed"))?;
            let status = response.status();
            let bytes = bounded_body(&mut response).await?;
            if !status.is_success() {
                return Err(io::Error::other("fixture server rejected request"));
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| io::Error::other("invalid fixture pipeline response"))
        })?;
        self.baton = response.baton;
        if response.results.len() != 1 {
            return Err(io::Error::other("unexpected fixture response count"));
        }
        match response.results.remove(0) {
            StreamResult::Ok { response } => Ok(response),
            _ => Err(io::Error::other("fixture SQL request failed")),
        }
    }
}

pub fn http_client() -> io::Result<reqwest::Client> {
    reqwest::Client::builder()
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(REQUEST_TIMEOUT)
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .build()
        .map_err(|_| io::Error::other("cannot create fixture HTTP client"))
}

pub async fn bounded_body(response: &mut reqwest::Response) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| io::Error::other("fixture response read failed"))?
    {
        if chunk.len() > BODY_LIMIT - bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "fixture response body exceeds limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
