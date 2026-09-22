//! What has to be configured before a staging certification run may start.
//!
//! Split out of `control_plane/services/staging.rs`, which had grown past the
//! module line cap.

use super::super::brama::BramaClient;
use super::super::contract::ContractError;
use super::super::transport::{ReqwestTransport, SecretRef};
use super::super::weles::WelesClient;
use sha2::{Digest, Sha256};
use std::time::Duration;

const REQUIRED_ENV: &[(&str, &str)] = &[
    ("BRAMA_STAGING_URL", "Brama staging HTTPS endpoint"),
    ("WELES_STAGING_URL", "Weles staging HTTPS endpoint"),
    (
        "JEDEN_STAGING_OIDC_TOKEN",
        "short-lived workload OIDC credential for the configured audience/role",
    ),
    ("JEDEN_STAGING_OIDC_AUDIENCE", "workload OIDC audience"),
    ("JEDEN_STAGING_OIDC_ROLE", "staging workload role"),
    (
        "JEDEN_STAGING_TENANT",
        "disposable staging tenant/account namespace",
    ),
    (
        "JEDEN_STAGING_PROVIDER",
        "provider enabled for disposable lifecycle",
    ),
    ("JEDEN_STAGING_MODEL", "harmless model route with quota"),
    (
        "JEDEN_STAGING_SCHEMA_MIN",
        "minimum supported staging schema version",
    ),
    (
        "JEDEN_STAGING_SCHEMA_MAX",
        "maximum supported staging schema version",
    ),
    (
        "JEDEN_STAGING_REPORT_SIGNING_KEY_HEX",
        "32-byte short-lived Ed25519 report signing seed",
    ),
    (
        "JEDEN_RELEASE_DIGEST",
        "immutable released canary digest under certification",
    ),
];

pub(super) fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

pub(super) fn identity(endpoint: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(endpoint.as_bytes()))
    )
}

pub fn staging_preflight_from_env() -> Result<(BramaClient, WelesClient), ContractError> {
    let prerequisites = REQUIRED_ENV
        .iter()
        .filter(|&(name, _detail)| required(name).is_empty())
        .map(|(name, detail)| format!("{name}: {detail}"))
        .collect::<Vec<_>>();
    if !prerequisites.is_empty() {
        return Err(ContractError::ExternalBlocked { prerequisites });
    }
    let brama = BramaClient::with_secret_ref(
        Some(required("BRAMA_STAGING_URL")),
        Some(SecretRef::environment("JEDEN_STAGING_OIDC_TOKEN")),
        Duration::from_secs(30),
        ReqwestTransport::production(),
    );
    let weles = WelesClient::with_secret_ref(
        Some(required("WELES_STAGING_URL")),
        Some(SecretRef::environment("JEDEN_STAGING_OIDC_TOKEN")),
        Duration::from_millis(500),
        ReqwestTransport::production(),
    );
    Ok((brama, weles))
}
