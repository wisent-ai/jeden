//! The signed record a staging certification run produces, and the check that
//! it was not altered afterwards.
//!
//! Split out of `control_plane/services/staging.rs`, which had grown past the
//! module line cap.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StagingEvidence {
    pub schema_version: u32,
    pub status: String,
    pub release_digest: String,
    pub endpoint_identities: Vec<String>,
    pub schema_revisions: Vec<String>,
    pub request_ids: Vec<String>,
    pub operation_ids: Vec<String>,
    pub served_route: String,
    pub usage_input_tokens: u64,
    pub usage_output_tokens: u64,
    pub redacted_trace_refs: Vec<String>,
    pub evidence_digest: String,
    pub signing_public_key: String,
    pub signing_key_id: String,
    pub signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UnsignedEvidence<'a> {
    pub(super) schema_version: u32,
    pub(super) status: &'a str,
    pub(super) release_digest: &'a str,
    pub(super) endpoint_identities: &'a [String],
    pub(super) schema_revisions: &'a [String],
    pub(super) request_ids: &'a [String],
    pub(super) operation_ids: &'a [String],
    pub(super) served_route: &'a str,
    pub(super) usage_input_tokens: u64,
    pub(super) usage_output_tokens: u64,
    pub(super) redacted_trace_refs: &'a [String],
    pub(super) signing_key_id: &'a str,
    pub(super) signing_public_key: &'a str,
}

pub fn verify_staging_report(evidence: &StagingEvidence) -> Result<(), String> {
    let unsigned = UnsignedEvidence {
        schema_version: evidence.schema_version,
        status: &evidence.status,
        release_digest: &evidence.release_digest,
        endpoint_identities: &evidence.endpoint_identities,
        schema_revisions: &evidence.schema_revisions,
        request_ids: &evidence.request_ids,
        operation_ids: &evidence.operation_ids,
        served_route: &evidence.served_route,
        usage_input_tokens: evidence.usage_input_tokens,
        usage_output_tokens: evidence.usage_output_tokens,
        redacted_trace_refs: &evidence.redacted_trace_refs,
        signing_key_id: &evidence.signing_key_id,
        signing_public_key: &evidence.signing_public_key,
    };
    let canonical = serde_json::to_vec(&unsigned).map_err(|error| error.to_string())?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&canonical)));
    if digest != evidence.evidence_digest {
        return Err("staging evidence digest mismatch".into());
    }
    let public: [u8; 32] = hex::decode(&evidence.signing_public_key)
        .map_err(|_| "invalid signing public key hex".to_string())?
        .try_into()
        .map_err(|_| "signing public key must be 32 bytes".to_string())?;
    let verifying = VerifyingKey::from_bytes(&public).map_err(|error| error.to_string())?;
    let expected_key_id = format!(
        "ed25519:{}",
        hex::encode(Sha256::digest(verifying.as_bytes()))
    );
    if expected_key_id != evidence.signing_key_id {
        return Err("staging signing key id mismatch".into());
    }
    let signature: [u8; 64] = hex::decode(&evidence.signature)
        .map_err(|_| "invalid signature hex".to_string())?
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    verifying
        .verify(&canonical, &Signature::from_bytes(&signature))
        .map_err(|error| error.to_string())
}
