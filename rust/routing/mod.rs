//! Deterministic, outcome-informed model routing.
//!
//! Route selection is intentionally separate from transport retry and failover. A
//! decision describes the route the policy selected; [`ServedRouteEvidence`]
//! records what the reliability layer actually served.

mod canary;
mod policy;
mod store;

pub use canary::{stable_cohort, CanaryControl, CanaryGuardrail};
pub use policy::{
    Candidate, CandidateEstimate, Constraints, DecisionMode, ReasonCode, RouteDecisionV1,
    RoutingPolicy, RoutingRequest,
};
pub use store::{
    OutcomeObservation, QualityMetrics, QualitySnapshot, QualityStore, RouteQuality,
    ServedRouteEvidence,
};
