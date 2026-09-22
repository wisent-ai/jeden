use super::manifest::{EnvelopeSignature, MarketplaceEnvelopeV1};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustRootV1 {
    pub version: u64,
    pub threshold: usize,
    pub expires_at: u64,
    pub keys: BTreeMap<String, String>,
    #[serde(default)]
    pub revoked_keys: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRotationV1 {
    pub previous_version: u64,
    pub next: TrustRootV1,
    pub signatures: Vec<EnvelopeSignature>,
}

fn decode_key(value: &str) -> Result<VerifyingKey, String> {
    let bytes = hex::decode(value).map_err(|_| "public key is not hex".to_string())?;
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "Ed25519 public key must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&key).map_err(|error| error.to_string())
}

fn verify_threshold(
    root: &TrustRootV1,
    payload: &[u8],
    signatures: &[EnvelopeSignature],
) -> Result<(), String> {
    if root.threshold == 0 || root.threshold > root.keys.len() {
        return Err("invalid trust-root threshold".into());
    }
    let mut valid = BTreeSet::new();
    for signed in signatures {
        if valid.contains(&signed.key_id) || root.revoked_keys.contains(&signed.key_id) {
            continue;
        }
        let Some(key) = root.keys.get(&signed.key_id) else {
            continue;
        };
        let signature_bytes = hex::decode(&signed.signature)
            .map_err(|_| format!("signature {} is not hex", signed.key_id))?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|error| error.to_string())?;
        if decode_key(key)?.verify(payload, &signature).is_ok() {
            valid.insert(signed.key_id.clone());
        }
    }
    if valid.len() < root.threshold {
        return Err(format!(
            "signature threshold not met: {}/{}",
            valid.len(),
            root.threshold
        ));
    }
    Ok(())
}

impl TrustRootV1 {
    pub fn verify_catalog(
        &self,
        envelope: &MarketplaceEnvelopeV1,
        now: u64,
        last_sequence: Option<u64>,
    ) -> Result<(), String> {
        envelope.validate_shape()?;
        if self.expires_at < now {
            return Err("marketplace trust root expired".into());
        }
        if envelope.root_version != self.version {
            return Err(format!(
                "catalog root version {} does not match trusted {}",
                envelope.root_version, self.version
            ));
        }
        if envelope.catalog.expires_at < now || envelope.catalog.issued_at > now {
            return Err("catalog is expired or not yet valid".into());
        }
        if last_sequence.is_some_and(|sequence| envelope.catalog.sequence <= sequence) {
            return Err("catalog replay/freeze detected".into());
        }
        let mut effective_root = self.clone();
        effective_root
            .revoked_keys
            .extend(envelope.catalog.revoked_keys.iter().cloned());
        verify_threshold(
            &effective_root,
            &envelope.signing_bytes()?,
            &envelope.signatures,
        )
    }

    pub fn rotate(&self, rotation: &RootRotationV1, now: u64) -> Result<TrustRootV1, String> {
        if self.expires_at < now {
            return Err("current trust root expired before rotation".into());
        }
        if rotation.previous_version != self.version
            || rotation.next.version != self.version.saturating_add(1)
        {
            return Err("non-monotonic trust-root rotation".into());
        }
        let payload = serde_json::to_vec(&rotation.next).map_err(|error| error.to_string())?;
        verify_threshold(self, &payload, &rotation.signatures)?;
        if rotation.next.threshold == 0
            || rotation.next.threshold > rotation.next.keys.len()
            || rotation.next.expires_at <= now
        {
            return Err("rotated trust root is unusable".into());
        }
        Ok(rotation.next.clone())
    }
}
