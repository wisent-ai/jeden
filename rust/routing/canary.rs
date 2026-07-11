use sha2::{Digest, Sha256};

/// Stable, non-PII cohort in the range `0..10_000` (basis points).
pub fn stable_cohort(subject_key: &str, salt: &str) -> u16 {
    let mut digest = Sha256::new();
    digest.update((salt.len() as u64).to_be_bytes());
    digest.update(salt.as_bytes());
    digest.update((subject_key.len() as u64).to_be_bytes());
    digest.update(subject_key.as_bytes());
    let bytes = digest.finalize();
    u64::from_be_bytes(bytes[..8].try_into().expect("SHA-256 prefix")).wrapping_rem(10_000) as u16
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanaryGuardrail {
    pub minimum_samples: u64,
    pub maximum_failure_rate: f64,
}

impl Default for CanaryGuardrail {
    fn default() -> Self {
        Self {
            minimum_samples: 20,
            maximum_failure_rate: 0.20,
        }
    }
}

/// Mutable operational state. Decisions only copy its immutable revision/state.
#[derive(Clone, Debug, PartialEq)]
pub struct CanaryControl {
    pub allocation_basis_points: u16,
    pub revision: String,
    pub killed: bool,
    pub guardrail: CanaryGuardrail,
    samples: u64,
    failures: u64,
}

impl CanaryControl {
    pub fn new(allocation_basis_points: u16, revision: impl Into<String>) -> Self {
        Self {
            allocation_basis_points: allocation_basis_points.min(10_000),
            revision: revision.into(),
            killed: false,
            guardrail: CanaryGuardrail::default(),
            samples: 0,
            failures: 0,
        }
    }

    pub fn is_canary(&self, subject_key: &str, salt: &str) -> bool {
        !self.killed && stable_cohort(subject_key, salt) < self.allocation_basis_points
    }

    pub fn kill(&mut self) {
        self.killed = true;
    }

    /// Returns true when this observation trips the guardrail and rolls back.
    pub fn observe(&mut self, succeeded: bool) -> bool {
        self.samples = self.samples.saturating_add(1);
        if !succeeded {
            self.failures = self.failures.saturating_add(1);
        }
        if self.samples >= self.guardrail.minimum_samples
            && self.failures as f64 / self.samples as f64 > self.guardrail.maximum_failure_rate
        {
            self.killed = true;
            true
        } else {
            false
        }
    }
}
