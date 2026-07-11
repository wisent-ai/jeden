use super::{stable_cohort, CanaryControl, QualityStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub route: String,
    pub capabilities: BTreeSet<String>,
    pub max_context_tokens: u64,
    pub estimated_cost_microunits: u64,
    pub estimated_latency_ms: u64,
    pub quality_prior: f64,
    pub enabled: bool,
    pub canary: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Constraints {
    pub required_capabilities: BTreeSet<String>,
    pub context_tokens: u64,
    pub max_cost_microunits: Option<u64>,
    pub max_latency_ms: Option<u64>,
    pub allowed_routes: Option<BTreeSet<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionMode {
    Serve,
    Shadow,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    Eligible,
    CapabilityExcluded,
    ContextExcluded,
    BudgetExcluded,
    LatencyExcluded,
    PolicyExcluded,
    Disabled,
    CanaryCohort,
    CanaryRollback,
    ColdStartPrior,
    OutcomePrior,
    Shadow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateEstimate {
    pub route: String,
    pub eligible: bool,
    pub predicted_quality: f64,
    pub predicted_cost_microunits: u64,
    pub predicted_latency_ms: u64,
    pub utility: f64,
    pub reasons: Vec<ReasonCode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutingRequest {
    pub subject_key: String,
    pub constraints: Constraints,
    pub mode: DecisionMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutingPolicy {
    pub revision: String,
    pub catalog_revision: String,
    pub cohort_salt: String,
    pub quality_weight: f64,
    pub cost_weight: f64,
    pub latency_weight: f64,
    pub outcome_prior_samples: f64,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            revision: "policy-v1".into(),
            catalog_revision: "catalog-v1".into(),
            cohort_salt: "routing-v1".into(),
            quality_weight: 1.0,
            cost_weight: 0.000_001,
            latency_weight: 0.000_1,
            outcome_prior_samples: 2.0,
        }
    }
}

/// Immutable evidence emitted before the reliability layer begins attempts.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteDecisionV1 {
    pub decision_id: String,
    pub selected_route: String,
    pub shadow_route: Option<String>,
    pub mode: DecisionMode,
    pub policy_revision: String,
    pub catalog_revision: String,
    pub canary_revision: String,
    pub cohort: u16,
    pub estimates: Vec<CandidateEstimate>,
}

impl RouteDecisionV1 {
    pub fn executes_selected_route(&self) -> bool {
        self.mode == DecisionMode::Serve
    }
}

impl RoutingPolicy {
    pub fn decide(
        &self,
        request: &RoutingRequest,
        catalog: &[Candidate],
        store: &QualityStore,
        canary: &CanaryControl,
    ) -> Result<RouteDecisionV1, String> {
        if self.revision.trim().is_empty() || self.catalog_revision.trim().is_empty() {
            return Err("routing revisions must be non-empty".into());
        }
        let cohort = stable_cohort(&request.subject_key, &self.cohort_salt);
        let canary_eligible = canary.is_canary(&request.subject_key, &self.cohort_salt);
        let mut estimates = Vec::with_capacity(catalog.len());
        for candidate in catalog {
            let mut reasons = Vec::new();
            if !candidate.enabled {
                reasons.push(ReasonCode::Disabled);
            }
            if !candidate
                .capabilities
                .is_superset(&request.constraints.required_capabilities)
            {
                reasons.push(ReasonCode::CapabilityExcluded);
            }
            if candidate.max_context_tokens < request.constraints.context_tokens {
                reasons.push(ReasonCode::ContextExcluded);
            }
            if request
                .constraints
                .max_cost_microunits
                .is_some_and(|limit| candidate.estimated_cost_microunits > limit)
            {
                reasons.push(ReasonCode::BudgetExcluded);
            }
            if request
                .constraints
                .max_latency_ms
                .is_some_and(|limit| candidate.estimated_latency_ms > limit)
            {
                reasons.push(ReasonCode::LatencyExcluded);
            }
            if request
                .constraints
                .allowed_routes
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&candidate.route))
            {
                reasons.push(ReasonCode::PolicyExcluded);
            }
            if candidate.canary && !canary_eligible {
                reasons.push(if canary.killed {
                    ReasonCode::CanaryRollback
                } else {
                    ReasonCode::PolicyExcluded
                });
            }
            let eligible = reasons.is_empty();
            let stats = store.route(&candidate.route);
            let (predicted_quality, predicted_cost_microunits, predicted_latency_ms) = match stats
                .as_ref()
            {
                Some(stats) if stats.samples > 0 => {
                    reasons.push(ReasonCode::OutcomePrior);
                    let denominator = self.outcome_prior_samples + stats.samples as f64;
                    (
                        (candidate.quality_prior * self.outcome_prior_samples
                            + stats.mean_quality * stats.samples as f64)
                            / denominator,
                        ((candidate.estimated_cost_microunits as f64 * self.outcome_prior_samples
                            + stats.mean_cost_microunits * stats.samples as f64)
                            / denominator)
                            .round() as u64,
                        ((candidate.estimated_latency_ms as f64 * self.outcome_prior_samples
                            + stats.mean_latency_ms * stats.samples as f64)
                            / denominator)
                            .round() as u64,
                    )
                }
                _ => {
                    reasons.push(ReasonCode::ColdStartPrior);
                    (
                        candidate.quality_prior.clamp(0.0, 1.0),
                        candidate.estimated_cost_microunits,
                        candidate.estimated_latency_ms,
                    )
                }
            };
            if eligible {
                reasons.push(ReasonCode::Eligible);
            }
            if candidate.canary && canary_eligible {
                reasons.push(ReasonCode::CanaryCohort);
            }
            if request.mode == DecisionMode::Shadow {
                reasons.push(ReasonCode::Shadow);
            }
            let utility = self.quality_weight * predicted_quality
                - self.cost_weight * predicted_cost_microunits as f64
                - self.latency_weight * predicted_latency_ms as f64;
            estimates.push(CandidateEstimate {
                route: candidate.route.clone(),
                eligible,
                predicted_quality,
                predicted_cost_microunits,
                predicted_latency_ms,
                utility,
                reasons,
            });
        }
        let selected = estimates
            .iter()
            .filter(|entry| entry.eligible)
            .max_by(|left, right| {
                left.utility
                    .partial_cmp(&right.utility)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| right.route.cmp(&left.route))
            })
            .ok_or_else(|| "no eligible routing candidate".to_string())?;
        let selected_route = selected.route.clone();
        let shadow_route = (request.mode == DecisionMode::Shadow).then(|| selected_route.clone());
        let mut digest = Sha256::new();
        for part in [
            &self.revision,
            &self.catalog_revision,
            &canary.revision,
            &request.subject_key,
            &selected_route,
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part.as_bytes());
        }
        digest.update(cohort.to_be_bytes());
        let decision_id = hex::encode(digest.finalize());
        Ok(RouteDecisionV1 {
            decision_id,
            selected_route,
            shadow_route,
            mode: request.mode.clone(),
            policy_revision: self.revision.clone(),
            catalog_revision: self.catalog_revision.clone(),
            canary_revision: canary.revision.clone(),
            cohort,
            estimates,
        })
    }
}
