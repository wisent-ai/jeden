//! What a signed release says about itself, and who is allowed to say it.
//!
//! Split out of `update/manifest.rs`, which had grown past the module line cap.

use base64::Engine;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

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
