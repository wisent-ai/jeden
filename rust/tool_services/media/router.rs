use super::super::types::{check_operation, ServiceError, ServiceResult};
use crate::tool_runtime::runtime_ops::OperationContext;
use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub(super) struct CapabilityStatus {
    pub(super) job_id: String,
    pub(super) status: String,
    pub(super) error: Option<String>,
}

pub(super) struct MediaRouterClient {
    origin: Url,
    token: String,
    client: Client,
}

impl MediaRouterClient {
    pub(super) fn configured() -> Result<Self, String> {
        let raw_origin = std::env::var("STADO_MEDIA_ROUTER_URL")
            .map_err(|_| "STADO_MEDIA_ROUTER_URL is required".to_string())?;
        let token = std::env::var("JEDEN_MEDIA_ROUTER_TOKEN")
            .map_err(|_| "JEDEN_MEDIA_ROUTER_TOKEN is required".to_string())?;
        if token.trim().is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err("JEDEN_MEDIA_ROUTER_TOKEN is empty or malformed".into());
        }
        let mut origin = Url::parse(raw_origin.trim())
            .map_err(|error| format!("invalid STADO_MEDIA_ROUTER_URL: {error}"))?;
        let loopback = matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        let insecure_loopback = std::env::var("STADO_MEDIA_ROUTER_ALLOW_INSECURE_LOOPBACK")
            .ok()
            .is_some_and(|value| value == "1");
        if origin.scheme() != "https"
            && !(origin.scheme() == "http" && loopback && insecure_loopback)
        {
            return Err(
                "STADO_MEDIA_ROUTER_URL must use HTTPS or explicitly enabled loopback HTTP".into(),
            );
        }
        if !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !matches!(origin.path(), "" | "/")
        {
            return Err(
                "STADO_MEDIA_ROUTER_URL must be an origin without credentials or path".into(),
            );
        }
        origin.set_path("/");
        let client = crate::net::blocking_builder()
            .build()
            .map_err(|error| format!("failed to initialize media-router client: {error}"))?;
        Ok(Self {
            origin,
            token,
            client,
        })
    }

    pub(super) fn endpoint(&self, path: &str) -> ServiceResult<Url> {
        self.origin
            .join(path)
            .map_err(|error| ServiceError::Protocol {
                service: "media-router",
                detail: error.to_string(),
            })
    }

    pub(super) fn post_json<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &T,
        context: &OperationContext<'_>,
    ) -> ServiceResult<R> {
        check_operation(context)?;
        let response = self
            .client
            .post(self.endpoint(path)?)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .map_err(|error| backend_error("media-router", error))?;
        if !response.status().is_success() {
            return Err(response_failure("media-router", response));
        }
        response.json().map_err(|error| ServiceError::Protocol {
            service: "media-router",
            detail: error.to_string(),
        })
    }

    pub(super) fn status(
        &self,
        job_id: &str,
        context: &OperationContext<'_>,
    ) -> ServiceResult<CapabilityStatus> {
        check_job_id(job_id)?;
        check_operation(context)?;
        let response = self
            .client
            .get(self.endpoint(&format!("media/{job_id}"))?)
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| backend_error("media-router", error))?;
        if !response.status().is_success() {
            return Err(response_failure("media-router", response));
        }
        response.json().map_err(|error| ServiceError::Protocol {
            service: "media-router",
            detail: error.to_string(),
        })
    }

    pub(super) fn content(
        &self,
        job_id: &str,
        expected_type: &str,
        context: &OperationContext<'_>,
    ) -> ServiceResult<(Vec<u8>, String)> {
        check_job_id(job_id)?;
        check_operation(context)?;
        let response = self
            .client
            .get(self.endpoint(&format!("media/{job_id}/content"))?)
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| backend_error("media-router", error))?;
        if !response.status().is_success() {
            return Err(response_failure("media-router", response));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ServiceError::Protocol {
                service: "media-router",
                detail: "media content response lacks Content-Type".into(),
            })?
            .to_ascii_lowercase();
        if !content_type.starts_with(expected_type) {
            return Err(ServiceError::Protocol {
                service: "media-router",
                detail: format!("expected {expected_type} content, received {content_type}"),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > "33554432".parse().expect("valid media output limit"))
        {
            return Err(ServiceError::OutputLimit {
                limit: "33554432".parse().expect("valid media output limit"),
            });
        }
        let bytes = response
            .bytes()
            .map_err(|error| backend_error("media-router", error))?
            .to_vec();
        if bytes.is_empty() {
            return Err(ServiceError::Protocol {
                service: "media-router",
                detail: "media content response was empty".into(),
            });
        }
        Ok((bytes, content_type))
    }
}

pub(super) fn check_job_id(job_id: &str) -> ServiceResult<()> {
    if job_id.is_empty()
        || !job_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ServiceError::Protocol {
            service: "media-router",
            detail: "media-router returned an invalid job_id".into(),
        });
    }
    Ok(())
}

fn response_failure(service: &'static str, response: reqwest::blocking::Response) -> ServiceError {
    let status = response.status();
    let detail = response
        .text()
        .map(|body| {
            body.chars()
                .take("512".parse().expect("valid error detail limit"))
                .collect::<String>()
        })
        .unwrap_or_else(|_| "response body unavailable".into());
    ServiceError::Backend {
        service,
        detail: format!("HTTP {status}: {detail}"),
    }
}

fn backend_error(service: &'static str, error: reqwest::Error) -> ServiceError {
    ServiceError::Backend {
        service,
        detail: error.to_string(),
    }
}
