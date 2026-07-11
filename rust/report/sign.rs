use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DsseSignature {
    pub keyid: String,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DsseEnvelope {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedRoot {
    pub ed25519_keys: BTreeMap<String, String>,
}

pub fn key_id(key: &VerifyingKey) -> String {
    format!("ed25519:{}", hex::encode(Sha256::digest(key.as_bytes())))
}

fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    format!(
        "DSSEv1 {} {} {} ",
        payload_type.len(),
        payload_type,
        payload.len()
    )
    .into_bytes()
    .into_iter()
    .chain(payload.iter().copied())
    .collect()
}

pub fn sign(payload_type: &str, payload: &[u8], key: &SigningKey) -> DsseEnvelope {
    let signature = key.sign(&pae(payload_type, payload));
    DsseEnvelope {
        payload_type: payload_type.to_owned(),
        payload: BASE64.encode(payload),
        signatures: vec![DsseSignature {
            keyid: key_id(&key.verifying_key()),
            sig: BASE64.encode(signature.to_bytes()),
        }],
    }
}

pub fn verify(envelope: &DsseEnvelope, root: &TrustedRoot) -> Result<Vec<u8>, String> {
    if envelope.signatures.len() != 1 {
        return Err("DSSE envelope must have exactly one signature".into());
    }
    let signed = &envelope.signatures[0];
    let public_hex = root
        .ed25519_keys
        .get(&signed.keyid)
        .ok_or_else(|| format!("untrusted DSSE key {}", signed.keyid))?;
    let public: [u8; 32] = hex::decode(public_hex)
        .map_err(|_| "trusted root contains invalid Ed25519 hex".to_string())?
        .try_into()
        .map_err(|_| "trusted Ed25519 key must be 32 bytes".to_string())?;
    let verifying = VerifyingKey::from_bytes(&public).map_err(|error| error.to_string())?;
    if key_id(&verifying) != signed.keyid {
        return Err("trusted root key id mismatch".into());
    }
    let signature: [u8; 64] = BASE64
        .decode(&signed.sig)
        .map_err(|_| "invalid DSSE signature base64".to_string())?
        .try_into()
        .map_err(|_| "Ed25519 signature must be 64 bytes".to_string())?;
    let payload = BASE64
        .decode(&envelope.payload)
        .map_err(|_| "invalid DSSE payload base64".to_string())?;
    verifying
        .verify(
            &pae(&envelope.payload_type, &payload),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| "DSSE signature verification failed".to_string())?;
    Ok(payload)
}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

pub fn envelope_digest(envelope: &DsseEnvelope) -> Result<String, String> {
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(canonical_json(envelope)?))
    ))
}
