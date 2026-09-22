//! Deciding whether a release manifest may be installed on this machine.
//!
//! Split out of `update/manifest.rs`, which had grown past the module line cap.

use super::instant::parse_rfc3339;
use super::types::ReleaseManifestV2;
use semver::Version;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn validate_hash(label: &str, value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label} must be a SHA-256 hex digest"));
    }
    if value != value.to_ascii_lowercase() {
        return Err(format!("{label} must use canonical lowercase hex"));
    }
    Ok(())
}

pub(super) fn validate_manifest(
    manifest: &ReleaseManifestV2,
    expected_channel: &str,
    expected_target: &str,
    current_version: &Version,
    now: Option<u64>,
) -> Result<(), String> {
    if manifest.schema_version != 2 {
        return Err(format!(
            "unsupported release manifest schema {}",
            manifest.schema_version
        ));
    }
    if !matches!(manifest.channel.as_str(), "canary" | "stable")
        || manifest.channel != expected_channel
    {
        return Err(format!(
            "release channel mismatch: expected {expected_channel}, got {}",
            manifest.channel
        ));
    }
    if manifest.target_triple != expected_target {
        return Err(format!(
            "release target mismatch: expected {expected_target}, got {}",
            manifest.target_triple
        ));
    }
    validate_hash("artifact sha256", &manifest.sha256)?;
    if manifest.size == 0 {
        return Err("release artifact size must be non-zero".into());
    }
    if manifest.artifact_url.is_empty()
        || manifest.provenance_ref.is_empty()
        || manifest.sbom_ref.is_empty()
    {
        return Err("release artifact, provenance, and SBOM references are required".into());
    }
    let candidate = Version::parse(&manifest.version)
        .map_err(|error| format!("invalid release version: {error}"))?;
    let minimum = Version::parse(&manifest.minimum_version)
        .map_err(|error| format!("invalid minimum version: {error}"))?;
    if &candidate <= current_version {
        return Err(format!(
            "release downgrade/replay refused: {candidate} is not newer than {current_version}"
        ));
    }
    if current_version < &minimum {
        return Err(format!(
            "current version {current_version} is below release minimum {minimum}"
        ));
    }
    let now = now.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    });
    let published_at = parse_rfc3339(&manifest.published_at)?;
    let expires_at = parse_rfc3339(&manifest.expires_at)?;
    if published_at > now.saturating_add(300) {
        return Err("release manifest publication time is in the future".into());
    }
    if expires_at <= now || expires_at <= published_at {
        return Err("release manifest is expired or has an invalid validity window".into());
    }
    Ok(())
}
