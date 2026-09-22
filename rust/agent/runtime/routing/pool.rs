//! The subscription pool this workshop can route a model call to, read
//! from platform billing rather than assumed.
//!
//! Split out of `agent/runtime/routing.rs`, which had grown past the module
//! line cap.

use crate::control_plane::billing::{AccountState, SubscriptionState, MAX_BILLING_ITEMS};
use crate::control_plane::contract::{RequestMeta, WelesApiV2};
use crate::control_plane::weles::{platform_billing_configured, WelesClient};
use crate::routing::subscriptions::SubscriptionTarget;
use crate::routing::SubscriptionPoolSnapshot;
use sha2::{Digest, Sha256};

pub(super) fn subscription_pool_from_platform_billing(
) -> Result<Option<SubscriptionPoolSnapshot>, String> {
    if !platform_billing_configured() {
        return Ok(None);
    }
    let client = WelesClient::from_env();
    let accounts = client.accounts(None).map_err(|error| {
        format!("cannot list platform billing accounts for subscription routing: {error}")
    })?;
    let mut targets = Vec::new();
    let mut revisions = Vec::new();
    'accounts: for (account_index, account) in accounts.into_iter().enumerate() {
        if account.status != "active" {
            continue;
        }
        let correlation = format!("subscription-discovery-{account_index}");
        let Ok(status) = client.billing_status(
            &account.id,
            &RequestMeta::read_v2(format!("{correlation}-status")),
        ) else {
            continue;
        };
        if status.status != AccountState::Active || status.provider_id != account.provider {
            continue;
        }
        let Ok(subscriptions) = client.subscriptions(
            &account.id,
            &RequestMeta::read_v2(format!("{correlation}-subscriptions")),
        ) else {
            continue;
        };
        for (subscription_index, subscription) in subscriptions.into_iter().enumerate() {
            if subscription.status != SubscriptionState::Active
                || subscription.provider_id != status.provider_id
            {
                continue;
            }
            let Ok(quota) = client.quota(
                &subscription.id,
                &RequestMeta::read_v2(format!("{correlation}-quota-{subscription_index}")),
            ) else {
                continue;
            };
            let limiting_bucket = quota.buckets.into_iter().min_by(|left, right| {
                let rank = |bucket: &crate::control_plane::billing::QuotaBucket| {
                    use crate::control_plane::billing::QuotaState;
                    match bucket.state {
                        QuotaState::Exhausted => 0_u8,
                        QuotaState::Unknown => 1,
                        QuotaState::Available => 2,
                        QuotaState::Unmetered => 3,
                    }
                };
                rank(left).cmp(&rank(right)).then_with(|| {
                    match (
                        left.remaining.zip(left.limit),
                        right.remaining.zip(right.limit),
                    ) {
                        (
                            Some((left_remaining, left_limit)),
                            Some((right_remaining, right_limit)),
                        ) if left_limit > 0 && right_limit > 0 => (u128::from(left_remaining)
                            * u128::from(right_limit))
                        .cmp(&(u128::from(right_remaining) * u128::from(left_limit))),
                        _ => left.bucket_id.cmp(&right.bucket_id),
                    }
                })
            });
            let Some(bucket) = limiting_bucket else {
                continue;
            };
            revisions.push(quota.revision);
            targets.push(SubscriptionTarget {
                provider_id: subscription.provider_id,
                account_id: account.id.clone(),
                subscription_id: subscription.id,
                quota_bucket: bucket.bucket_id,
                priority: 0,
                quota_state: bucket.state,
                remaining: bucket.remaining,
                limit: bucket.limit,
                capabilities: ["chat".to_string()].into_iter().collect(),
                active: true,
                valid_until_ms: u64::MAX,
                policy_allowed: true,
            });
            if targets.len() >= MAX_BILLING_ITEMS {
                break 'accounts;
            }
        }
    }
    if targets.is_empty() {
        return Ok(None);
    }
    targets.sort_by_key(SubscriptionTarget::identity);
    revisions.sort();
    revisions.dedup();
    let encoded = serde_json::to_vec(&(revisions, &targets)).map_err(|error| error.to_string())?;
    let revision = hex::encode(Sha256::digest(encoded));
    Ok(Some(SubscriptionPoolSnapshot {
        revision,
        rendezvous_salt: "weles-subscription-routing-v1".into(),
        targets,
    }))
}
