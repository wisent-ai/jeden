use super::*;
use std::collections::BTreeSet;

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).into()).collect()
}
fn candidate(route: &str, quality: f64) -> Candidate {
    Candidate {
        route: route.into(),
        capabilities: set(&["tools"]),
        max_context_tokens: 8_000,
        estimated_cost_microunits: 10,
        estimated_latency_ms: 10,
        quality_prior: quality,
        enabled: true,
        canary: false,
    }
}
fn request(subject: &str) -> RoutingRequest {
    RoutingRequest {
        subject_key: subject.into(),
        constraints: Constraints::default(),
        mode: DecisionMode::Serve,
    }
}

#[test]
fn hard_constraints_exclude_capability_context_and_budget() {
    let policy = RoutingPolicy::default();
    let store = QualityStore::new();
    let canary = CanaryControl::new(0, "c1");
    let mut candidates = vec![
        candidate("cap", 0.9),
        candidate("context", 0.8),
        candidate("budget", 0.7),
        candidate("ok", 0.6),
    ];
    candidates[0].capabilities.clear();
    candidates[1].max_context_tokens = 10;
    candidates[2].estimated_cost_microunits = 101;
    let mut req = request("a");
    req.constraints.required_capabilities = set(&["tools"]);
    req.constraints.context_tokens = 100;
    req.constraints.max_cost_microunits = Some(100);
    let decision = policy.decide(&req, &candidates, &store, &canary).unwrap();
    assert_eq!(decision.selected_route, "ok");
    assert!(decision
        .estimates
        .iter()
        .find(|e| e.route == "cap")
        .unwrap()
        .reasons
        .contains(&ReasonCode::CapabilityExcluded));
    assert!(decision
        .estimates
        .iter()
        .find(|e| e.route == "context")
        .unwrap()
        .reasons
        .contains(&ReasonCode::ContextExcluded));
    assert!(decision
        .estimates
        .iter()
        .find(|e| e.route == "budget")
        .unwrap()
        .reasons
        .contains(&ReasonCode::BudgetExcluded));
}

#[test]
fn cold_start_and_cohort_are_deterministic_across_retry() {
    let policy = RoutingPolicy::default();
    let store = QualityStore::new();
    let canary = CanaryControl::new(10_000, "c1");
    let req = request("stable-subject");
    let candidates = vec![candidate("b", 0.5), candidate("a", 0.5)];
    let first = policy.decide(&req, &candidates, &store, &canary).unwrap();
    let retry = policy.decide(&req, &candidates, &store, &canary).unwrap();
    assert_eq!(first.selected_route, "a");
    assert_eq!(first.decision_id, retry.decision_id);
    assert_eq!(first.cohort, retry.cohort);
}

#[test]
fn outcomes_shift_choice_and_attribute_served_fallback() {
    let policy = RoutingPolicy::default();
    let store = QualityStore::new();
    let canary = CanaryControl::new(0, "c1");
    let req = request("a");
    let candidates = vec![candidate("primary", 0.7), candidate("fallback", 0.6)];
    assert_eq!(
        policy
            .decide(&req, &candidates, &store, &canary)
            .unwrap()
            .selected_route,
        "primary"
    );
    for _ in 0..10 {
        store.update(OutcomeObservation {
            served_route: "fallback".into(),
            succeeded: true,
            quality: 1.0,
            cost_microunits: 10,
            latency_ms: 10,
            retries: 1,
            failovers: 1,
        });
    }
    for _ in 0..10 {
        store.update(OutcomeObservation {
            served_route: "primary".into(),
            succeeded: false,
            quality: 0.0,
            cost_microunits: 10,
            latency_ms: 10,
            retries: 0,
            failovers: 0,
        });
    }
    assert_eq!(
        policy
            .decide(&req, &candidates, &store, &canary)
            .unwrap()
            .selected_route,
        "fallback"
    );
    let evidence = ServedRouteEvidence::initial("d", "primary").fallback("fallback", 2);
    assert_eq!(evidence.served_route, "fallback");
    assert!(evidence.fallback && evidence.retry);
    assert_eq!(store.route("fallback").unwrap().samples, 10);
}

#[test]
fn kill_switch_and_guardrail_roll_back_canary() {
    let mut policy = RoutingPolicy::default();
    policy.cohort_salt = "s".into();
    let store = QualityStore::new();
    let mut canary = CanaryControl::new(10_000, "c1");
    canary.guardrail = CanaryGuardrail {
        minimum_samples: 2,
        maximum_failure_rate: 0.4,
    };
    let mut baseline = candidate("baseline", 0.5);
    let mut experimental = candidate("canary", 0.9);
    experimental.canary = true;
    assert_eq!(
        policy
            .decide(
                &request("x"),
                &[baseline.clone(), experimental.clone()],
                &store,
                &canary
            )
            .unwrap()
            .selected_route,
        "canary"
    );
    assert!(!canary.observe(false));
    assert!(canary.observe(false));
    assert_eq!(
        policy
            .decide(
                &request("x"),
                &[baseline.clone(), experimental],
                &store,
                &canary
            )
            .unwrap()
            .selected_route,
        "baseline"
    );
    baseline.canary = true;
    assert!(policy
        .decide(&request("x"), &[baseline], &store, &canary)
        .is_err());
}

#[test]
fn shadow_has_no_execution_and_metrics_are_reported() {
    let policy = RoutingPolicy::default();
    let store = QualityStore::new();
    let canary = CanaryControl::new(0, "c1");
    let mut req = request("x");
    req.mode = DecisionMode::Shadow;
    let decision = policy
        .decide(&req, &[candidate("route", 0.8)], &store, &canary)
        .unwrap();
    assert!(!decision.executes_selected_route());
    assert_eq!(decision.shadow_route.as_deref(), Some("route"));
    store.update(OutcomeObservation {
        served_route: "route".into(),
        succeeded: true,
        quality: 0.8,
        cost_microunits: 1,
        latency_ms: 2,
        retries: 0,
        failovers: 0,
    });
    let metrics = store.snapshot().metrics;
    assert_eq!(metrics.observations, 1);
    assert!(metrics.mean_regret >= 0.0);
    assert!(metrics.calibration_error >= 0.0);
}

#[test]
fn model_transport_attributes_retry_and_fallback_to_served_route() {
    use crate::model_router::{Completion, RouteDescriptor, RouteResult, StreamingCompletion};
    let policy = RoutingPolicy::default();
    let decision = policy
        .decide(
            &request("x"),
            &[candidate("primary", 0.8)],
            &QualityStore::new(),
            &CanaryControl::new(0, "c1"),
        )
        .unwrap();
    let primary = RouteDescriptor {
        model: "primary".into(),
        service_tier: None,
    };
    let fallback = RouteDescriptor {
        model: "fallback".into(),
        service_tier: None,
    };
    let completion = StreamingCompletion {
        completion: Completion {
            content: "ok".into(),
            usage: None,
        },
        route: fallback.clone(),
        route_results: vec![
            RouteResult::RetryScheduled {
                route: primary.clone(),
                attempt: 1,
                delay_ms: 0,
                reason: "transient".into(),
            },
            RouteResult::RouteChanged {
                from: primary,
                to: fallback,
                reason: "failover".into(),
            },
        ],
        subscription_target: None,
        subscription_decision_id: None,
    };
    let evidence = completion.served_route_evidence(&decision);
    assert_eq!(evidence.selected_route, "primary");
    assert_eq!(evidence.served_route, "fallback");
    assert_eq!(evidence.attempt, 2);
    assert!(evidence.retry && evidence.fallback);
}
