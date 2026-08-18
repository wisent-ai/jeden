use reqwest::blocking::Client;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub enum SecretRef {
    Environment(String),
    Inline(String),
}

impl SecretRef {
    pub fn environment(name: impl Into<String>) -> Self {
        Self::Environment(name.into())
    }
    pub fn inline(value: impl Into<String>) -> Self {
        Self::Inline(value.into())
    }
    pub fn resolve(&self) -> Option<String> {
        match self {
            Self::Environment(name) => std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty()),
            Self::Inline(value) => (!value.trim().is_empty()).then(|| value.clone()),
        }
    }
}

impl std::fmt::Debug for SecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretRef([REDACTED])")
    }
}

#[derive(Clone)]
pub struct TransportRequest {
    pub method: reqwest::Method,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub max_response_bytes: u64,
}

impl std::fmt::Debug for TransportRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .field("max_response_bytes", &self.max_response_bytes)
            .finish()
    }
}

#[derive(Clone)]
pub struct TransportResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for TransportResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportResponse")
            .field("status", &self.status)
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

pub trait ControlPlaneTransport: Send + Sync {
    fn execute(&self, request: TransportRequest) -> Result<TransportResponse, String>;
}

#[derive(Clone)]
pub struct ReqwestTransport {
    client: Client,
}
impl ReqwestTransport {
    pub fn production() -> Arc<dyn ControlPlaneTransport> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| Client::new());
        Arc::new(Self { client })
    }
}
/// Render an error together with everything underneath it.
fn describe(error: reqwest::Error) -> String {
    let mut rendered = error.to_string();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&error);
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

impl ControlPlaneTransport for ReqwestTransport {
    fn execute(&self, request: TransportRequest) -> Result<TransportResponse, String> {
        let mut builder = self.client.request(request.method, request.url);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder
                .header("content-type", "application/json")
                .body(body);
        }
        // `reqwest::Error`'s own Display stops at "error sending request for
        // url (...)", which names the destination and withholds the reason. The
        // cause chain underneath distinguishes a refused connect from a timeout
        // from a closed connection, and without it every one of them reads as
        // the network being down.
        let response = builder.send().map_err(describe)?;
        if response
            .content_length()
            .is_some_and(|length| length > request.max_response_bytes)
        {
            return Err("response exceeds negotiated payload limit".into());
        }
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect();
        let mut body = Vec::new();
        response
            .take(request.max_response_bytes + 1)
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        if body.len() as u64 > request.max_response_bytes {
            return Err("response exceeds negotiated payload limit".into());
        }
        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }
}
