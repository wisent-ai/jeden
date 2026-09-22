//! Translating between the shapes the billing service speaks and the shapes
//! this command surface shows, in one place so the two cannot drift.
//!
//! Split out of `cli/reports/billing.rs`, which had grown past the module line
//! cap.

use super::super::model::{
    BillingError, BillingPolicy, SubscriptionMutationResult, SubscriptionSummary,
};
use crate::control_plane::billing::PolicyPeriod;
use crate::control_plane::billing::{PaymentMethodReference, PurchasePolicy, SubscriptionState};
use crate::control_plane::contract::{RequestMeta, WelesApiV2};
use crate::control_plane::weles::WelesClient;

pub(super) fn correlation(action: &str, identity: &str) -> String {
    format!("jeden-{action}-{identity}")
}

/// The one payment method a purchase may charge. None and several are both
/// refused: choosing between methods is the operator's call, made in Weles.
pub(super) fn sole_payment_method(
    client: &WelesClient,
    account_id: &str,
) -> Result<PaymentMethodReference, BillingError> {
    let methods = client
        .payment_methods(
            account_id,
            &RequestMeta::read_v2(correlation("payment-methods", account_id)),
        )
        .map_err(backend)?;
    match methods.as_slice() {
        [method] => Ok(method.clone()),
        [] => Err(BillingError::NotFound(format!(
            "account `{account_id}` has no payment method; run /payment-method setup"
        ))),
        _ => Err(BillingError::Backend(format!(
            "account `{account_id}` has multiple payment methods; choose one in Weles"
        ))),
    }
}
pub(super) fn policy_from_weles(policy: PurchasePolicy) -> BillingPolicy {
    BillingPolicy::new(
        policy.enabled,
        policy.auto_renew,
        policy.allowed_products,
        policy.allowed_currencies,
        policy.max_single_microunits,
        policy.max_period_microunits,
        match policy.period {
            PolicyPeriod::Day => "day",
            PolicyPeriod::Month => "month",
            PolicyPeriod::BillingCycle => "billing-cycle",
        },
        policy.revision,
        policy.valid_until_ms,
    )
}
pub(super) fn policy_to_weles(policy: BillingPolicy) -> PurchasePolicy {
    PurchasePolicy {
        enabled: policy.enabled,
        auto_renew: policy.auto_renew,
        allowed_products: policy.allowed_products,
        allowed_currencies: policy.allowed_currencies,
        max_single_microunits: policy.max_single_microunits,
        max_period_microunits: policy.max_period_microunits,
        period: match policy.period.as_str() {
            "day" => PolicyPeriod::Day,
            "billing-cycle" => PolicyPeriod::BillingCycle,
            _ => PolicyPeriod::Month,
        },
        revision: policy.revision,
        valid_until_ms: policy.valid_until_ms,
    }
}
pub(super) fn state_text(state: SubscriptionState) -> &'static str {
    match state {
        SubscriptionState::Active => "active",
        SubscriptionState::PastDue => "past_due",
        SubscriptionState::Paused => "paused",
        SubscriptionState::Cancelled => "cancelled",
        SubscriptionState::Expired => "expired",
    }
}
pub(super) fn summary(
    subscription: crate::control_plane::billing::SubscriptionV2,
) -> SubscriptionSummary {
    SubscriptionSummary::new(
        subscription.id,
        subscription.account_id,
        subscription.provider_id,
        subscription.product_id,
        state_text(subscription.status),
        subscription.renews_at_ms,
    )
}
pub(super) fn operation(
    result: crate::control_plane::billing::OperationResult,
) -> SubscriptionMutationResult {
    SubscriptionMutationResult::new(
        result.operation_id,
        format!("{:?}", result.state).to_ascii_lowercase(),
    )
}
pub(super) fn backend(error: crate::control_plane::weles::WelesError) -> BillingError {
    BillingError::Backend(error.to_string())
}
