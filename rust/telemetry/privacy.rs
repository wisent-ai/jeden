use sha2::{Digest, Sha256};

use super::schema::{CorrelationIds, PrivateId};

/// Per-install pseudonymizer. The salt must be stored locally and must not be exported.
#[derive(Clone)]
pub struct PrivacyFilter {
    salt: [u8; 32],
}

impl std::fmt::Debug for PrivacyFilter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivacyFilter")
            .finish_non_exhaustive()
    }
}

impl PrivacyFilter {
    pub fn new(salt: [u8; 32]) -> Self {
        Self { salt }
    }

    /// Produces a stable local pseudonym without retaining or serializing `raw`.
    pub fn pseudonymize(&self, namespace: &'static str, raw: &str) -> PrivateId {
        let mut digest = Sha256::new();
        digest.update(b"jeden.private-telemetry.v1\0");
        digest.update(self.salt);
        digest.update(namespace.as_bytes());
        digest.update([0]);
        digest.update(raw.as_bytes());
        PrivateId::from_digest(format!("pid_{}", hex::encode(digest.finalize())))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn correlation_ids(
        &self,
        operation_id: &str,
        session_id: &str,
        parent_operation_id: Option<&str>,
        attempt_id: Option<&str>,
        route_id: Option<&str>,
        capability_id: Option<&str>,
        capability_generation: Option<u64>,
    ) -> CorrelationIds {
        CorrelationIds {
            operation_id: self.pseudonymize("operation", operation_id),
            session_id: self.pseudonymize("session", session_id),
            parent_operation_id: parent_operation_id
                .map(|value| self.pseudonymize("operation", value)),
            attempt_id: attempt_id.map(|value| self.pseudonymize("attempt", value)),
            route_id: route_id.map(|value| self.pseudonymize("route", value)),
            capability_id: capability_id.map(|value| self.pseudonymize("capability", value)),
            capability_generation,
        }
    }
}

/// Returns false if any exact canary byte sequence reached a durable/captured representation.
/// Empty canaries are ignored so callers can safely pass optional fixture values.
pub fn contains_canary(haystack: &[u8], canaries: &[&str]) -> bool {
    canaries
        .iter()
        .filter(|value| !value.is_empty())
        .any(|canary| {
            haystack
                .windows(canary.len())
                .any(|window| window == canary.as_bytes())
        })
}
