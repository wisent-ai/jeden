use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PAYLOAD_TYPE: &str = "application/vnd.jeden.release-manifest.v2+json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseManifestV2 {
    pub schema_version: u32,
    pub version: String,
    pub channel: String,
    pub target_triple: String,
    pub artifact_url: String,
    pub sha256: String,
    pub size: u64,
    pub published_at: String,
    pub expires_at: String,
    pub minimum_version: String,
    pub key_id: String,
    pub provenance_ref: String,
    pub sbom_ref: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DsseEnvelope {
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DsseSignature {
    pub keyid: String,
    pub sig: String,
}

#[derive(Clone, Debug)]
pub struct TrustRoot {
    pub channel: String,
    pub key_id: String,
    pub key: VerifyingKey,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustRootDocument {
    pub schema_version: u32,
    pub roots: Vec<TrustRootEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustRootEntry {
    pub channel: String,
    pub key_id: String,
    pub public_key: String,
}

impl TrustRootDocument {
    pub fn decode(self) -> Result<Vec<TrustRoot>, String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported trust-root schema {}",
                self.schema_version
            ));
        }
        self.roots
            .into_iter()
            .map(|entry| {
                if !matches!(entry.channel.as_str(), "canary" | "stable") {
                    return Err(format!("unsupported trust-root channel {}", entry.channel));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&entry.public_key)
                    .map_err(|_| format!("trust root {} is not base64", entry.key_id))?;
                let bytes: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| format!("trust root {} must be 32 bytes", entry.key_id))?;
                let key = VerifyingKey::from_bytes(&bytes).map_err(|_| {
                    format!("trust root {} is not an Ed25519 public key", entry.key_id)
                })?;
                Ok(TrustRoot {
                    channel: entry.channel,
                    key_id: entry.key_id,
                    key,
                })
            })
            .collect()
    }
}

fn pae(payload_type: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = format!("DSSEv1 {} ", payload_type.len()).into_bytes();
    out.extend_from_slice(payload_type);
    out.extend_from_slice(format!(" {} ", payload.len()).as_bytes());
    out.extend_from_slice(payload);
    out
}

fn validate_hash(label: &str, value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label} must be a SHA-256 hex digest"));
    }
    if value != value.to_ascii_lowercase() {
        return Err(format!("{label} must use canonical lowercase hex"));
    }
    Ok(())
}

fn canonical_payload(manifest: &ReleaseManifestV2) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(manifest).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or("release manifest payload is not an object")?;
    let ordered: BTreeMap<&str, &serde_json::Value> = object
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    serde_json::to_vec(&ordered).map_err(|error| error.to_string())
}

pub fn verify_envelope(
    bytes: &[u8],
    roots: &[TrustRoot],
    expected_channel: &str,
    expected_target: &str,
    current_version: &Version,
    now: Option<u64>,
) -> Result<ReleaseManifestV2, String> {
    let envelope: DsseEnvelope =
        serde_json::from_slice(bytes).map_err(|error| format!("invalid DSSE envelope: {error}"))?;
    if envelope.payload_type != PAYLOAD_TYPE {
        return Err(format!(
            "unsupported DSSE payload type {}",
            envelope.payload_type
        ));
    }
    let payload = base64::engine::general_purpose::STANDARD
        .decode(&envelope.payload)
        .map_err(|_| "DSSE payload is not canonical base64".to_string())?;
    if base64::engine::general_purpose::STANDARD.encode(&payload) != envelope.payload {
        return Err("DSSE payload is not canonical base64".into());
    }
    let manifest: ReleaseManifestV2 = serde_json::from_slice(&payload)
        .map_err(|error| format!("invalid release manifest payload: {error}"))?;
    if canonical_payload(&manifest)? != payload {
        return Err("release manifest payload is not canonical JSON".into());
    }
    validate_manifest(
        &manifest,
        expected_channel,
        expected_target,
        current_version,
        now,
    )?;
    let root = roots
        .iter()
        .find(|root| root.channel == manifest.channel && root.key_id == manifest.key_id)
        .ok_or_else(|| {
            format!(
                "untrusted release key {} for channel {}",
                manifest.key_id, manifest.channel
            )
        })?;
    let signature = envelope
        .signatures
        .iter()
        .find(|signature| signature.keyid == manifest.key_id)
        .ok_or_else(|| format!("DSSE envelope has no signature from {}", manifest.key_id))?;
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(&signature.sig)
        .map_err(|_| "DSSE signature is not base64".to_string())?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| "DSSE signature must be 64 bytes".to_string())?;
    root.key
        .verify(&pae(envelope.payload_type.as_bytes(), &payload), &signature)
        .map_err(|_| "release manifest Ed25519 signature verification failed".to_string())?;
    Ok(manifest)
}

fn parse_rfc3339(value: &str) -> Result<u64, String> {
    if value.len() < 20
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
    {
        return Err(format!("timestamp is not RFC3339: {value}"));
    }
    let number = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .and_then(|part| part.parse::<i64>().ok())
            .ok_or_else(|| format!("timestamp is not RFC3339: {value}"))
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(format!("timestamp is not RFC3339: {value}"));
    }
    let suffix = &value[19..];
    let offset = if suffix == "Z" {
        0
    } else if suffix.len() == 6
        && matches!(suffix.as_bytes()[0], b'+' | b'-')
        && suffix.as_bytes()[3] == b':'
    {
        let hours = suffix[1..3]
            .parse::<i64>()
            .map_err(|_| format!("timestamp is not RFC3339: {value}"))?;
        let minutes = suffix[4..6]
            .parse::<i64>()
            .map_err(|_| format!("timestamp is not RFC3339: {value}"))?;
        if hours > 23 || minutes > 59 {
            return Err(format!("timestamp is not RFC3339: {value}"));
        }
        let seconds = hours * 3600 + minutes * 60;
        if suffix.starts_with('-') {
            -seconds
        } else {
            seconds
        }
    } else {
        return Err(format!("timestamp is not RFC3339: {value}"));
    };
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let timestamp = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(timestamp).map_err(|_| format!("timestamp predates Unix epoch: {value}"))
}

fn validate_manifest(
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

pub fn verify_artifact(manifest: &ReleaseManifestV2, artifact: &[u8]) -> Result<(), String> {
    if artifact.len() as u64 != manifest.size {
        return Err(format!(
            "update size mismatch: expected {}, got {}",
            manifest.size,
            artifact.len()
        ));
    }
    let actual = hex::encode(Sha256::digest(artifact));
    if actual != manifest.sha256 {
        return Err(format!(
            "update checksum mismatch: expected {}, got {actual}",
            manifest.sha256
        ));
    }
    Ok(())
}
