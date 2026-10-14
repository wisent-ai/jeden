//! What has to be configured before a staging certification run may start.
//!
//! Split out of `control_plane/services/staging.rs`, which had grown past the
//! module line cap.

use super::super::brama::BramaClient;
use super::super::contract::ContractError;
use super::super::transport::{ReqwestTransport, SecretRef};
use super::super::weles::WelesClient;
use sha2::{Digest, Sha256};

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

/// A configured prerequisite's value. One that is unset, empty or not text
/// is refused with its name, never read as an empty value.
pub(super) fn required(name: &str) -> Result<String, String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Ok(_) => Err(format!("{name} is set but empty")),
        Err(error) => Err(format!("{name} cannot be read: {error}")),
    }
}

pub(super) fn identity(endpoint: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(endpoint.as_bytes()))
    )
}

pub fn staging_preflight_from_env() -> Result<(BramaClient, WelesClient), ContractError> {
    // A value that is not text is named as such: read as empty it was listed
    // as missing, and the operator was sent to set a variable already set.
    let prerequisites = REQUIRED_ENV
        .iter()
        .filter_map(|(name, detail)| match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => None,
            Ok(_) | Err(std::env::VarError::NotPresent) => Some(format!("{name}: {detail}")),
            Err(error) => Some(format!("{name}: {detail} (set, but {error})")),
        })
        .collect::<Vec<_>>();
    if !prerequisites.is_empty() {
        return Err(ContractError::ExternalBlocked { prerequisites });
    }
    let endpoint = |name: &str| {
        required(name).map_err(|cause| ContractError::ExternalBlocked {
            prerequisites: vec![cause],
        })
    };
    let brama = BramaClient::with_secret_ref(
        Some(endpoint("BRAMA_STAGING_URL")?),
        Some(SecretRef::environment("JEDEN_STAGING_OIDC_TOKEN")),
        None,
        ReqwestTransport::production(),
    );
    let weles = WelesClient::with_secret_ref(
        Some(endpoint("WELES_STAGING_URL")?),
        Some(SecretRef::environment("JEDEN_STAGING_OIDC_TOKEN")),
        ReqwestTransport::production(),
    );
    Ok((brama, weles))
}
