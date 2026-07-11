use super::subscriptions::*;
use std::collections::BTreeSet;

fn target(
    subscription: &str,
    bucket: &str,
    priority: u32,
    remaining: u64,
    limit: u64,
) -> SubscriptionTarget {
    SubscriptionTarget {
        provider_id: "provider".into(),
        account_id: format!("account-{subscription}"),
        subscription_id: subscription.into(),
        quota_bucket: bucket.into(),
        priority,
        remaining,
        limit,
        capabilities: ["chat".to_string()].into_iter().collect(),
        active: true,
        policy_allowed: true,
        valid_until_ms: u64::MAX,
    }
}

fn snapshot(targets: Vec<SubscriptionTarget>) -> SubscriptionPoolSnapshot {
    SubscriptionPoolSnapshot {
        revision: "revision-7".into(),
        rendezvous_salt: "stable-salt".into(),
        targets,
    }
}

fn required() -> BTreeSet<String> {
    ["chat".to_string()].into_iter().collect()
}

#[test]
fn ordering_is_deterministic_priority_first_and_sticky_for_exact_ties() {
    let pool = snapshot(vec![
        target("low-priority", "chat", 9, 100, 100),
        target("ratio-low", "chat", 1, 25, 100),
        target("tie-a", "chat", 1, 75, 100),
        target("tie-b", "chat", 1, 75, 100),
    ]);

    let first = pool
        .ordered_targets("session-42", &required(), 0, |_| false)
        .unwrap();
    let repeated = pool
        .ordered_targets("session-42", &required(), 0, |_| false)
        .unwrap();
    let reversed = snapshot(pool.targets.iter().cloned().rev().collect())
        .ordered_targets("session-42", &required(), 0, |_| false)
        .unwrap();

    assert_eq!(first, repeated, "retry must not perturb sticky order");
    assert_eq!(
        first, reversed,
        "catalog input order must not affect routing"
    );
    assert!(matches!(
        first[0].subscription_id.as_str(),
        "tie-a" | "tie-b"
    ));
    assert!(matches!(
        first[1].subscription_id.as_str(),
        "tie-a" | "tie-b"
    ));
    assert_ne!(first[0].subscription_id, first[1].subscription_id);
    assert_eq!(first[2].subscription_id, "ratio-low");
    assert_eq!(first[3].subscription_id, "low-priority");
}

#[test]
fn freeze_pins_revision_order_and_caller_idempotency() {
    let mut source = snapshot(vec![
        target("primary", "chat", 1, 90, 100),
        target("secondary", "chat", 2, 100, 100),
    ]);
    let decision = RouteDecisionV2::freeze(
        &source,
        "request-1",
        "caller-stable-idempotency",
        "session-1",
        &required(),
        0,
        |_| false,
    )
    .unwrap();
    let repeated = RouteDecisionV2::freeze(
        &source,
        "request-1",
        "caller-stable-idempotency",
        "session-1",
        &required(),
        0,
        |_| false,
    )
    .unwrap();

    source.revision = "revision-8".into();
    source.targets.reverse();
    source.targets[0].priority = 0;

    assert_eq!(decision, repeated);
    assert_eq!(decision.snapshot_revision, "revision-7");
    assert_eq!(decision.idempotency_key, "caller-stable-idempotency");
    assert_eq!(decision.selected.subscription_id, "primary");
    assert_eq!(decision.targets[0].subscription_id, "primary");
    assert_eq!(decision.targets[1].subscription_id, "secondary");
}

#[test]
fn eligibility_rejects_inactive_policy_capability_quota_and_cooldown() {
    let required = required();
    let mut value = target("subscription", "chat", 1, 1, 1);
    assert_eq!(
        SubscriptionPoolSnapshot::eligibility(&value, &required, false, 0),
        SubscriptionEligibility::Eligible
    );

    value.active = false;
    assert_eq!(
        SubscriptionPoolSnapshot::eligibility(&value, &required, false, 0),
        SubscriptionEligibility::Inactive
    );
    value.active = true;
    value.policy_allowed = false;
    assert_eq!(
        SubscriptionPoolSnapshot::eligibility(&value, &required, false, 0),
        SubscriptionEligibility::PolicyExcluded
    );
    value.policy_allowed = true;
    value.capabilities.clear();
    assert_eq!(
        SubscriptionPoolSnapshot::eligibility(&value, &required, false, 0),
        SubscriptionEligibility::CapabilityExcluded
    );
    value.capabilities = required.clone();
    value.remaining = 0;
    assert_eq!(
        SubscriptionPoolSnapshot::eligibility(&value, &required, false, 0),
        SubscriptionEligibility::QuotaExhausted
    );
    value.remaining = 1;
    assert_eq!(
        SubscriptionPoolSnapshot::eligibility(&value, &required, true, 0),
        SubscriptionEligibility::CoolingDown
    );
}

#[test]
fn cooldown_is_scoped_to_exact_subscription_and_bucket() {
    let first = target("shared", "requests", 1, 100, 100);
    let second_bucket = target("shared", "tokens", 1, 100, 100);
    let other_subscription = target("other", "requests", 1, 100, 100);
    let cooling_identity = first.identity();
    let ordered = snapshot(vec![first, second_bucket, other_subscription])
        .ordered_targets("session", &required(), 0, |identity| {
            identity == &cooling_identity
        })
        .unwrap();

    assert_eq!(ordered.len(), 2);
    assert!(ordered
        .iter()
        .any(|item| item.subscription_id == "shared" && item.quota_bucket == "tokens"));
    assert!(ordered
        .iter()
        .any(|item| item.subscription_id == "other" && item.quota_bucket == "requests"));
}

#[test]
fn invalid_snapshot_and_empty_logical_keys_fail_closed() {
    let duplicate = target("same", "chat", 1, 1, 1);
    assert!(snapshot(vec![duplicate.clone(), duplicate])
        .validate()
        .is_err());
    assert!(RouteDecisionV2::freeze(
        &snapshot(vec![target("one", "chat", 1, 1, 1)]),
        "",
        "idem",
        "sticky",
        &required(),
        0,
        |_| false
    )
    .is_err());
    assert!(RouteDecisionV2::freeze(
        &snapshot(vec![target("one", "chat", 1, 1, 1)]),
        "request",
        "",
        "sticky",
        &required(),
        0,
        |_| false
    )
    .is_err());
    assert!(RouteDecisionV2::freeze(
        &snapshot(vec![target("one", "chat", 1, 1, 1)]),
        "request",
        "idem",
        "",
        &required(),
        0,
        |_| false
    )
    .is_err());
}
