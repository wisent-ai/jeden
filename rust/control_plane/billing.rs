use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

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

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PaymentMethodReference(String);
impl PaymentMethodReference {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for PaymentMethodReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PaymentMethodReference([REDACTED])")
    }
}
impl Serialize for PaymentMethodReference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for PaymentMethodReference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        validate_opaque_reference(&value).map_err(de::Error::custom)?;
        Ok(Self(value))
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BillingGrant(String);
impl BillingGrant {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for BillingGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BillingGrant([REDACTED])")
    }
}
impl Serialize for BillingGrant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for BillingGrant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        validate_opaque_reference(&value).map_err(de::Error::custom)?;
        Ok(Self(value))
    }
}

fn validate_opaque_reference(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("opaque reference is empty");
    }
    if value.len() > MAX_BILLING_STRING_BYTES {
        return Err("opaque reference is too long");
    }
    if value.chars().any(char::is_control) {
        return Err("opaque reference contains control characters");
    }
    Ok(())
}

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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaBucket {
    pub bucket_id: String,
    pub limit: u64,
    pub remaining: u64,
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
