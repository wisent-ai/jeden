use reqwest::blocking::Client;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

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
    /// The value behind this reference, or nothing when it is unset or blank.
    /// An unreadable process value refuses before a request is dispatched.
    pub fn resolve(&self) -> Result<Option<String>, String> {
        crate::agent::credential::ensure();
        match self {
            Self::Environment(name) => match std::env::var(name) {
                Ok(value) => Ok((!value.trim().is_empty()).then_some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => {
                    Err(format!("{name} cannot be read: value is not Unicode"))
                }
            },
            Self::Inline(value) => Ok((!value.trim().is_empty()).then(|| value.clone())),
        }
    }
    pub fn resolve_optional(reference: Option<&Self>) -> Result<Option<String>, String> {
        match reference {
            Some(reference) => reference.resolve(),
            None => Ok(None),
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
}

impl std::fmt::Debug for TransportRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
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
    client: Result<Client, String>,
}
impl ReqwestTransport {
    /// The control-plane client. A call ends when the control plane answers
    /// or the connection does; no interval here decides that for it.
    pub fn production() -> Arc<dyn ControlPlaneTransport> {
        let client = crate::net::blocking_builder()
            .build()
            .map_err(describe_reqwest);
        Arc::new(Self { client })
    }
}
/// Render an error together with everything underneath it.
pub(crate) fn describe_reqwest(error: reqwest::Error) -> String {
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
        let client = self
            .client
            .as_ref()
            .map_err(|error| format!("create HTTP client for {}: {error}", request.url))?;
        let mut builder = client.request(request.method, request.url);
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
        // cause chain underneath distinguishes a refused connect from a closed
        // connection, and without it every one of them reads as the network
        // being down.
        let response = builder.send().map_err(describe_reqwest)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let text = value
                    .to_str()
                    .map_err(|error| format!("read response header {name}: {error}"))?;
                Ok((name.as_str().to_ascii_lowercase(), text.to_owned()))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let mut body = Vec::new();
        let mut response = response;
        response
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }
}
