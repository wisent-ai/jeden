use super::transport::{ControlPlaneTransport, ReqwestTransport, SecretRef};
use super::{now_ms, ServiceHealth};
use std::sync::Arc;
use std::time::Duration;

pub(super) const API_VERSION: &str = "v1";
const PLATFORM_BILLING_URL_ENV: &str = "WISENT_PLATFORM_BILLING_URL";
const PLATFORM_BILLING_TOKEN_ENV: &str = "WISENT_PLATFORM_BILLING_TOKEN";
/// The query a holding read of an operation carries: the server answers once
/// the operation has events after the cursor or has ended, not before.
pub(super) const HOLD_QUERY: &str = "wait=1";
pub(super) const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
pub(super) const MAX_PROVIDERS: usize = 128;
pub(super) const MAX_ACCOUNTS: usize = 512;

mod contract;
mod login;
mod requests;
mod types;

pub use types::{Account, InteractionBridge, OperationEvent, OperationV1, Provider, WelesError};

#[derive(Clone)]
pub struct WelesClient {
    pub(super) endpoint: Option<String>,
    pub(super) authorization: Option<SecretRef>,
    pub(super) transport: Arc<dyn ControlPlaneTransport>,
    pub(super) correlation: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) fn platform_billing_configured() -> bool {
    std::env::var(PLATFORM_BILLING_URL_ENV)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn platform_billing_endpoint() -> Option<String> {
    std::env::var(PLATFORM_BILLING_URL_ENV).ok()
}

fn platform_billing_token() -> SecretRef {
    SecretRef::environment(PLATFORM_BILLING_TOKEN_ENV)
}

impl WelesClient {
    pub fn from_env() -> Self {
        Self::with_secret_ref(
            platform_billing_endpoint(),
            Some(platform_billing_token()),
            ReqwestTransport::production(),
        )
    }

    pub fn new(endpoint: Option<String>, bearer: Option<String>) -> Self {
        Self::with_transport(endpoint, bearer, ReqwestTransport::production())
    }

    pub fn with_transport(
        endpoint: Option<String>,
        bearer: Option<String>,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        Self::with_secret_ref(endpoint, bearer.map(SecretRef::inline), transport)
    }

    pub fn with_secret_ref(
        endpoint: Option<String>,
        authorization: Option<SecretRef>,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        let endpoint = endpoint
            .map(|v| v.trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty());
        Self {
            endpoint,
            authorization,
            transport,
            correlation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    pub(super) fn endpoint(&self) -> Result<&str, WelesError> {
        self.endpoint.as_deref().ok_or(WelesError::Unconfigured)
    }

    pub fn health(&self) -> ServiceHealth {
        let available = self.endpoint.is_some();
        ServiceHealth {
            service: "platform-billing".into(),
            version: API_VERSION.into(),
            available,
            endpoint: self.endpoint.clone(),
            detail: if available {
                "configured; provider state is resolved on demand".into()
            } else {
                "WISENT_PLATFORM_BILLING_URL is not configured".into()
            },
            checked_at_ms: now_ms(),
        }
    }

    pub(super) fn validate_hosted_setup(
        &self,
        setup: &super::billing::HostedPaymentSetup,
    ) -> Result<(), WelesError> {
        let hosted = url::Url::parse(&setup.hosted_url)
            .map_err(|_| WelesError::InvalidResponse("hosted setup URL is invalid".into()))?;
        if hosted.scheme() != "https" || hosted.host_str().is_none() {
            return Err(WelesError::InvalidResponse(
                "hosted setup URL must use HTTPS".into(),
            ));
        }
        let hosted_origin = hosted.origin().ascii_serialization();
        let endpoint = url::Url::parse(self.endpoint()?)
            .map_err(|_| WelesError::InvalidResponse("Weles endpoint URL is invalid".into()))?;
        let endpoint_origin = endpoint.origin().ascii_serialization();
        let configured = std::env::var("WELES_HOSTED_ORIGINS").unwrap_or_default();
        if hosted_origin != endpoint_origin
            && !configured
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .any(|origin| origin == hosted_origin)
        {
            return Err(WelesError::InvalidResponse(
                "hosted setup URL origin is not allowlisted".into(),
            ));
        }
        Ok(())
    }
}

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}
