use crate::control_plane::billing::QuotaState;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeSet;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionTarget {
    pub provider_id: String,
    pub account_id: String,
    pub subscription_id: String,
    pub quota_bucket: String,
    pub priority: u32,
    pub quota_state: QuotaState,
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    pub active: bool,
    /// Exclusive epoch-millisecond expiry supplied by the authority.
    pub valid_until_ms: u64,
    #[serde(default = "default_policy_allowed")]
    pub policy_allowed: bool,
}

fn default_policy_allowed() -> bool {
    true
}

impl SubscriptionTarget {
    pub fn identity(&self) -> SubscriptionTargetIdentity {
        SubscriptionTargetIdentity {
            provider_id: self.provider_id.clone(),
            account_id: self.account_id.clone(),
            subscription_id: self.subscription_id.clone(),
            quota_bucket: self.quota_bucket.clone(),
        }
    }
    fn quota_rank(&self) -> u8 {
        match self.quota_state {
            QuotaState::Exhausted => 0,
            QuotaState::Unknown => 1,
            QuotaState::Available => 2,
            QuotaState::Unmetered => 3,
        }
    }

    fn remaining_ratio(&self) -> Option<f64> {
        match (self.remaining, self.limit) {
            (Some(remaining), Some(limit)) if limit > 0 => Some(remaining as f64 / limit as f64),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionTargetIdentity {
    pub provider_id: String,
    pub account_id: String,
    pub subscription_id: String,
    pub quota_bucket: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionEligibility {
    Eligible,
    Inactive,
    Expired,
    CapabilityExcluded,
    PolicyExcluded,
    QuotaExhausted,
    CoolingDown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionPoolSnapshot {
    pub revision: String,
    pub rendezvous_salt: String,
    pub targets: Vec<SubscriptionTarget>,
}

impl SubscriptionPoolSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.revision.trim().is_empty() || self.rendezvous_salt.trim().is_empty() {
            return Err(
                "subscription snapshot revision and rendezvous salt must be non-empty".into(),
            );
        }
        let mut identities = BTreeSet::new();
        for target in &self.targets {
            if target.provider_id.trim().is_empty()
                || target.account_id.trim().is_empty()
                || target.subscription_id.trim().is_empty()
                || target.quota_bucket.trim().is_empty()
            {
                return Err("subscription target identity fields must be non-empty".into());
            }
            if !identities.insert(target.identity()) {
                return Err("subscription snapshot contains duplicate target identity".into());
            }
        }
        Ok(())
    }

    pub fn eligibility(
        target: &SubscriptionTarget,
        required: &BTreeSet<String>,
        cooling_down: bool,
        now_ms: u64,
    ) -> SubscriptionEligibility {
        if !target.active {
            SubscriptionEligibility::Inactive
        } else if target.valid_until_ms <= now_ms {
            SubscriptionEligibility::Expired
        } else if !target.policy_allowed {
            SubscriptionEligibility::PolicyExcluded
        } else if !target.capabilities.is_superset(required) {
            SubscriptionEligibility::CapabilityExcluded
        } else if target.quota_state == QuotaState::Exhausted
            || matches!(
                (target.remaining, target.limit),
                (Some(0), _) | (_, Some(0))
            )
        {
            SubscriptionEligibility::QuotaExhausted
        } else if cooling_down {
            SubscriptionEligibility::CoolingDown
        } else {
            SubscriptionEligibility::Eligible
        }
    }

    /// Freezes an eligible attempt order. Priority and remaining ratio dominate;
    /// rendezvous gives equal candidates stable affinity and lexical identity is
    /// the total-order tie breaker.
    pub fn ordered_targets<F>(
        &self,
        sticky_key: &str,
        required: &BTreeSet<String>,
        now_ms: u64,
        mut cooling_down: F,
    ) -> Result<Vec<SubscriptionTarget>, String>
    where
        F: FnMut(&SubscriptionTargetIdentity) -> bool,
    {
        self.validate()?;
        let mut targets: Vec<_> = self
            .targets
            .iter()
            .filter(|target| {
                Self::eligibility(target, required, cooling_down(&target.identity()), now_ms)
                    == SubscriptionEligibility::Eligible
            })
            .cloned()
            .collect();
        targets.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| right.quota_rank().cmp(&left.quota_rank()))
                .then_with(|| {
                    right
                        .remaining_ratio()
                        .partial_cmp(&left.remaining_ratio())
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| {
                    rendezvous_score(sticky_key, &self.rendezvous_salt, right)
                        .cmp(&rendezvous_score(sticky_key, &self.rendezvous_salt, left))
                })
                .then_with(|| left.provider_id.cmp(&right.provider_id))
                .then_with(|| left.account_id.cmp(&right.account_id))
                .then_with(|| left.subscription_id.cmp(&right.subscription_id))
                .then_with(|| left.quota_bucket.cmp(&right.quota_bucket))
        });
        Ok(targets)
    }
}

fn rendezvous_score(sticky_key: &str, salt: &str, target: &SubscriptionTarget) -> [u8; 32] {
    let mut mac =
        HmacSha256::new_from_slice(salt.as_bytes()).expect("HMAC accepts arbitrary key length");
    for part in [
        sticky_key,
        &target.provider_id,
        &target.account_id,
        &target.subscription_id,
        &target.quota_bucket,
    ] {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part.as_bytes());
    }
    mac.finalize().into_bytes().into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteDecisionV2 {
    pub decision_id: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub snapshot_revision: String,
    pub selected: SubscriptionTarget,
    /// Frozen failover order; never recomputed during the operation.
    pub targets: Vec<SubscriptionTarget>,
}

impl RouteDecisionV2 {
    pub fn freeze(
        snapshot: &SubscriptionPoolSnapshot,
        request_id: impl Into<String>,
        idempotency_key: impl Into<String>,
        sticky_key: &str,
        required: &BTreeSet<String>,
        now_ms: u64,
        cooling_down: impl FnMut(&SubscriptionTargetIdentity) -> bool,
    ) -> Result<Self, String> {
        let request_id = request_id.into();
        let idempotency_key = idempotency_key.into();
        if request_id.trim().is_empty()
            || idempotency_key.trim().is_empty()
            || sticky_key.trim().is_empty()
        {
            return Err("request, idempotency, and sticky keys must be non-empty".into());
        }
        let targets = snapshot.ordered_targets(sticky_key, required, now_ms, cooling_down)?;
        let selected = targets
            .first()
            .cloned()
            .ok_or("no eligible subscription target")?;
        let encoded = serde_json::to_vec(&(
            snapshot.revision.as_str(),
            &request_id,
            &idempotency_key,
            &targets,
        ))
        .map_err(|error| error.to_string())?;
        Ok(Self {
            decision_id: hex::encode(Sha256::digest(encoded)),
            request_id,
            idempotency_key,
            snapshot_revision: snapshot.revision.clone(),
            selected,
            targets,
        })
    }
}
