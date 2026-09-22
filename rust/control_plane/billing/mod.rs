//! The billing contract's own shapes: what an account, a policy, a
//! subscription, a quota and an operation result look like.

use serde::{Deserialize, Serialize};

pub const MAX_BILLING_ITEMS: usize = 512;
pub const MAX_BILLING_STRING_BYTES: usize = 2_048;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    Active,
    ActionRequired,
    Suspended,
    Closed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BillingCapability {
    HostedPaymentSetup,
    PurchasePolicy,
    Quote,
    Purchase,
    AutoRenew,
    SubscriptionManagement,
}

mod secrets;

pub use secrets::{BillingGrant, PaymentMethodReference};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountStatus {
    pub account_id: String,
    pub provider_id: String,
    pub status: AccountState,
    #[serde(default)]
    pub capabilities: Vec<BillingCapability>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyPeriod {
    Day,
    Month,
    BillingCycle,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurchasePolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_renew: bool,
    pub allowed_products: Vec<String>,
    pub allowed_currencies: Vec<String>,
    pub max_single_microunits: u64,
    pub max_period_microunits: u64,
    pub period: PolicyPeriod,
    pub revision: String,
    pub valid_until_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Active,
    PastDue,
    Paused,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionV2 {
    pub id: String,
    pub account_id: String,
    pub provider_id: String,
    pub product_id: String,
    pub status: SubscriptionState,
    #[serde(default)]
    pub renews_at_ms: Option<u64>,
    #[serde(default)]
    pub payment_method_reference: Option<PaymentMethodReference>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaState {
    Available,
    Exhausted,
    #[default]
    Unknown,
    Unmetered,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaBucket {
    pub bucket_id: String,
    #[serde(default)]
    pub state: QuotaState,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub remaining: Option<u64>,
    #[serde(default)]
    pub resets_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaSnapshot {
    pub subscription_id: String,
    pub revision: String,
    pub observed_at_ms: u64,
    pub buckets: Vec<QuotaBucket>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuoteRequest {
    pub account_id: String,
    pub provider_id: String,
    pub product_id: String,
    pub currency: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Quote {
    pub id: String,
    pub revision: String,
    pub account_id: String,
    pub provider_id: String,
    pub product_id: String,
    pub currency: String,
    pub amount_microunits: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentMethodSetupRequest {
    pub account_id: String,
    pub return_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostedPaymentSetup {
    pub setup_id: String,
    pub hosted_url: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurchaseRequest {
    pub quote_id: String,
    pub quote_revision: String,
    pub policy_revision: String,
    pub payment_method_reference: PaymentMethodReference,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenewRequest {
    pub quote_id: String,
    pub quote_revision: String,
    pub policy_revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Pending,
    Succeeded,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BillingErrorCode {
    PolicyDisabled,
    PolicyExpired,
    PolicyRevisionMismatch,
    QuoteExpired,
    QuoteRevisionMismatch,
    ProductDenied,
    CurrencyDenied,
    SingleLimitExceeded,
    PeriodLimitExceeded,
    PaymentMethodUnavailable,
    Conflict,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BillingOperationError {
    pub code: BillingErrorCode,
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
    #[serde(default)]
    pub current_policy_revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BillingEvent {
    PolicyApplied { revision: String },
    GrantIssued { grant: BillingGrant },
    SubscriptionChanged { subscription: SubscriptionV2 },
    PaymentMethodRevoked { reference: PaymentMethodReference },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationResult {
    pub operation_id: String,
    pub state: OperationState,
    #[serde(default)]
    pub events: Vec<BillingEvent>,
    #[serde(default)]
    pub error: Option<BillingOperationError>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BramaBillingSnapshot {
    pub provider_id: String,
    pub account_id: String,
    pub subscription_id: String,
    pub quota: QuotaSnapshot,
}

pub(crate) fn validate_policy(policy: &PurchasePolicy) -> Result<(), &'static str> {
    if !policy.enabled && policy.auto_renew {
        return Err("auto-renew requires an enabled purchase policy");
    }
    if policy.allowed_products.is_empty() || policy.allowed_products.len() > MAX_BILLING_ITEMS {
        return Err("allowed product count is invalid");
    }
    if policy.allowed_currencies.is_empty() || policy.allowed_currencies.len() > MAX_BILLING_ITEMS {
        return Err("allowed currency count is invalid");
    }
    if policy.max_single_microunits > policy.max_period_microunits {
        return Err("single purchase limit exceeds period limit");
    }
    if policy.revision.is_empty() || policy.revision.len() > MAX_BILLING_STRING_BYTES {
        return Err("policy revision is invalid");
    }
    Ok(())
}
