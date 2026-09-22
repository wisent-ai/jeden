//! What a billing command asks for and what comes back, declared as their own
//! shapes so nothing travels as loose text.
//!
//! Split out of `cli/reports/billing.rs`, which had grown past the module line
//! cap.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BillingCommand {
    PaymentMethodSetup {
        account_id: String,
    },
    PolicyGet {
        account_id: String,
    },
    PolicySet {
        account_id: String,
        policy: BillingPolicy,
        approval: PolicyApproval,
    },
    PolicyReset {
        account_id: String,
        approval: PolicyApproval,
    },
    SubscriptionsList {
        account_id: String,
    },
    SubscriptionStatus {
        account_id: String,
        subscription_id: String,
    },
    SubscriptionDisable {
        account_id: String,
        subscription_id: String,
        request: MutationRequest,
    },
    SubscriptionPurchase {
        account_id: String,
        request: PurchaseRequest,
    },
    SubscriptionRenew {
        account_id: String,
        subscription_id: String,
        request: MutationRequest,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaymentMethodSetup {
    pub(crate) hosted_url: String,
}
impl PaymentMethodSetup {
    pub(crate) fn new(hosted_url: impl Into<String>) -> Self {
        Self {
            hosted_url: hosted_url.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BillingPolicy {
    pub(crate) enabled: bool,
    pub(crate) auto_renew: bool,
    pub(crate) allowed_products: Vec<String>,
    pub(crate) allowed_currencies: Vec<String>,
    pub(crate) max_single_microunits: u64,
    pub(crate) max_period_microunits: u64,
    pub(crate) period: String,
    pub(crate) revision: String,
    pub(crate) valid_until_ms: u64,
}
impl BillingPolicy {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        enabled: bool,
        auto_renew: bool,
        allowed_products: Vec<String>,
        allowed_currencies: Vec<String>,
        max_single_microunits: u64,
        max_period_microunits: u64,
        period: impl Into<String>,
        revision: impl Into<String>,
        valid_until_ms: u64,
    ) -> Self {
        Self {
            enabled,
            auto_renew,
            allowed_products,
            allowed_currencies,
            max_single_microunits,
            max_period_microunits,
            period: period.into(),
            revision: revision.into(),
            valid_until_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PolicyApproval {
    pub(crate) approved: bool,
}
impl PolicyApproval {
    pub(crate) fn new(approved: bool) -> Self {
        Self { approved }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MutationRequest {
    pub(crate) idempotency_key: String,
    pub(crate) approved: bool,
}
impl MutationRequest {
    pub(crate) fn new(idempotency_key: impl Into<String>, approved: bool) -> Self {
        Self {
            idempotency_key: idempotency_key.into(),
            approved,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PurchaseRequest {
    pub(crate) product_id: String,
    pub(crate) currency: String,
    pub(crate) mutation: MutationRequest,
}
impl PurchaseRequest {
    pub(crate) fn new(
        product_id: impl Into<String>,
        currency: impl Into<String>,
        idempotency_key: impl Into<String>,
        approved: bool,
    ) -> Self {
        Self {
            product_id: product_id.into(),
            currency: currency.into(),
            mutation: MutationRequest::new(idempotency_key, approved),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscriptionSummary {
    pub(crate) id: String,
    pub(crate) account_id: String,
    pub(crate) provider_id: String,
    pub(crate) product_id: String,
    pub(crate) status: String,
    pub(crate) renews_at_ms: Option<u64>,
}
impl SubscriptionSummary {
    pub(crate) fn new(
        id: impl Into<String>,
        account_id: impl Into<String>,
        provider_id: impl Into<String>,
        product_id: impl Into<String>,
        status: impl Into<String>,
        renews_at_ms: Option<u64>,
    ) -> Self {
        Self {
            id: id.into(),
            account_id: account_id.into(),
            provider_id: provider_id.into(),
            product_id: product_id.into(),
            status: status.into(),
            renews_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuotaSummary {
    pub(crate) bucket_id: String,
    pub(crate) state: crate::control_plane::billing::QuotaState,
    pub(crate) limit: Option<u64>,
    pub(crate) remaining: Option<u64>,
    pub(crate) resets_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscriptionStatus {
    pub(crate) subscription: SubscriptionSummary,
    pub(crate) quota_revision: String,
    pub(crate) quota_observed_at_ms: u64,
    pub(crate) quota: Vec<QuotaSummary>,
}
impl SubscriptionStatus {
    pub(crate) fn new(
        subscription: SubscriptionSummary,
        quota_revision: impl Into<String>,
        quota_observed_at_ms: u64,
        quota: Vec<QuotaSummary>,
    ) -> Self {
        Self {
            subscription,
            quota_revision: quota_revision.into(),
            quota_observed_at_ms,
            quota,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscriptionMutationResult {
    pub(crate) operation_id: String,
    pub(crate) state: String,
}
impl SubscriptionMutationResult {
    pub(crate) fn new(operation_id: impl Into<String>, state: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            state: state.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BillingError {
    InvalidCommand(String),
    ApprovalRequired(String),
    NotFound(String),
    Backend(String),
}
impl std::fmt::Display for BillingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCommand(message)
            | Self::ApprovalRequired(message)
            | Self::NotFound(message)
            | Self::Backend(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for BillingError {}

pub(crate) trait BillingBackend {
    fn payment_method_setup(&self, account_id: &str) -> Result<PaymentMethodSetup, BillingError>;
    fn policy_get(&self, account_id: &str) -> Result<BillingPolicy, BillingError>;
    fn policy_set(
        &self,
        account_id: &str,
        policy: BillingPolicy,
        approval: PolicyApproval,
    ) -> Result<BillingPolicy, BillingError>;
    fn policy_reset(&self, account_id: &str) -> Result<BillingPolicy, BillingError>;
    fn subscriptions_list(
        &self,
        account_id: &str,
    ) -> Result<Vec<SubscriptionSummary>, BillingError>;
    fn subscription_status(
        &self,
        account_id: &str,
        subscription_id: &str,
    ) -> Result<SubscriptionStatus, BillingError>;
    fn subscription_disable(
        &self,
        account_id: &str,
        subscription_id: &str,
        request: MutationRequest,
    ) -> Result<SubscriptionMutationResult, BillingError>;
    fn subscription_purchase(
        &self,
        account_id: &str,
        request: PurchaseRequest,
    ) -> Result<SubscriptionMutationResult, BillingError>;
    fn subscription_renew(
        &self,
        account_id: &str,
        subscription_id: &str,
        request: MutationRequest,
    ) -> Result<SubscriptionMutationResult, BillingError>;
}
