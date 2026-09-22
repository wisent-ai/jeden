//! Verifying a signed release manifest before anything from it is installed.

use base64::Engine;
use ed25519_dalek::{Signature, Verifier};
use semver::Version;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod checks;
mod instant;
mod types;

use checks::validate_manifest;
pub use types::{
    DsseEnvelope, DsseSignature, ReleaseManifestV2, TrustRoot, TrustRootDocument, TrustRootEntry,
};

pub const PAYLOAD_TYPE: &str = "application/vnd.jeden.release-manifest.v2+json";

fn pae(payload_type: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = format!("DSSEv1 {} ", payload_type.len()).into_bytes();
    out.extend_from_slice(payload_type);
    out.extend_from_slice(format!(" {} ", payload.len()).as_bytes());
    out.extend_from_slice(payload);
    out
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
