use super::transport::{ControlPlaneTransport, ReqwestTransport, SecretRef};
use super::ServiceHealth;
use std::sync::Arc;

pub(super) const API_VERSION: &str = "v1";
const PLATFORM_BILLING_URL_ENV: &str = "WISENT_PLATFORM_BILLING_URL";
const PLATFORM_BILLING_TOKEN_ENV: &str = "WISENT_PLATFORM_BILLING_TOKEN";
/// The query a holding read of an operation carries: the server answers once
/// the operation has events after the cursor or has ended, not before.
pub(super) const HOLD_QUERY: &str = "wait=1";

mod contract;
mod login;
mod requests;
mod types;

pub use types::{
    Account, InteractionBridge, OperationEvent, OperationState, OperationV1, Provider, WelesError,
};

#[derive(Clone)]
pub struct WelesClient {
    pub(super) endpoint: Option<String>,
    pub(super) authorization: Option<SecretRef>,
    pub(super) transport: Arc<dyn ControlPlaneTransport>,
    pub(super) correlation: Arc<std::sync::atomic::AtomicU64>,
}

/// The billing endpoint the environment states; one that is not text is
/// refused at startup with its name.
fn platform_billing_endpoint() -> Option<String> {
    match std::env::var(PLATFORM_BILLING_URL_ENV) {
        Ok(endpoint) => Some(endpoint),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => panic!("{PLATFORM_BILLING_URL_ENV} cannot be read: {error}"),
    }
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
            correlation: Arc::new(std::sync::atomic::AtomicU64::new(crate::tally::FIRST)),
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
        }
    }

    pub(super) fn validate_hosted_setup(
        &self,
        setup: &super::billing::HostedPaymentSetup,
    ) -> Result<(), WelesError> {
        let hosted = url::Url::parse(&setup.hosted_url).map_err(|error| {
            WelesError::InvalidResponse(format!("hosted setup URL is invalid: {error}"))
        })?;
        if hosted.scheme() != "https" || hosted.host_str().is_none() {
            return Err(WelesError::InvalidResponse(
                "hosted setup URL must use HTTPS".into(),
            ));
        }
        let hosted_origin = hosted.origin().ascii_serialization();
        let endpoint = url::Url::parse(self.endpoint()?).map_err(|error| {
            WelesError::InvalidResponse(format!("Weles endpoint URL is invalid: {error}"))
        })?;
        let endpoint_origin = endpoint.origin().ascii_serialization();
        // An allowlist that is not text is refused by name: read as empty it
        // refused every other origin as "not allowlisted" without saying the
        // list could not be read.
        let configured = match std::env::var("WELES_HOSTED_ORIGINS") {
            Ok(configured) => configured,
            Err(std::env::VarError::NotPresent) => String::new(),
            Err(error) => {
                return Err(WelesError::InvalidResponse(format!(
                    "WELES_HOSTED_ORIGINS cannot be read: {error}"
                )))
            }
        };
        if hosted_origin != endpoint_origin
            && !configured
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .any(|origin| origin == hosted_origin)
        {
            return Err(WelesError::InvalidResponse(format!(
                "hosted setup URL origin {hosted_origin} is neither the Weles endpoint's origin \
                 {endpoint_origin} nor listed in WELES_HOSTED_ORIGINS"
            )));
        }
        Ok(())
    }
}

/// RFC 3986's unreserved characters stay as they are; every other byte of a
/// path segment is percent-encoded.
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn encode_path_segment(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, SEGMENT).to_string()
}
