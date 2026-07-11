use super::{brama, weles, ServiceHealth};
use serde::{Deserialize, Serialize};

pub const MIN_SCHEMA_VERSION: u32 = 1;
pub const MAX_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestMeta {
    pub correlation_id: String,
    pub idempotency_key: Option<String>,
    pub schema_min: u32,
    pub schema_max: u32,
}

impl RequestMeta {
    pub fn read(correlation_id: impl Into<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            idempotency_key: None,
            schema_min: 1,
            schema_max: 1,
        }
    }

    pub fn mutation(correlation_id: impl Into<String>, idempotency_key: impl Into<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            idempotency_key: Some(idempotency_key.into()),
            schema_min: 1,
            schema_max: 1,
        }
    }

    pub fn read_v2(correlation_id: impl Into<String>) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            idempotency_key: None,
            schema_min: 2,
            schema_max: 2,
        }
    }

    pub fn mutation_v2(
        correlation_id: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            correlation_id: correlation_id.into(),
            idempotency_key: Some(idempotency_key.into()),
            schema_min: 2,
            schema_max: 2,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub ready: bool,
    pub schema_min: u32,
    pub schema_max: u32,
    pub max_payload_bytes: u64,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    ExternalBlocked { prerequisites: Vec<String> },
    SchemaSkew { service_min: u32, service_max: u32 },
    Unauthorized,
    Forbidden,
    Conflict,
    RateLimited { retry_after_ms: Option<u64> },
    Unavailable { status: u16 },
    Malformed(String),
    Oversize,
    Timeout,
    ExpiredOperation,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteRequest {
    pub model: String,
    #[serde(default)]
    pub required_modalities: Vec<String>,
    #[serde(default)]
    pub requires_tools: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequest {
    pub route: String,
    pub prompt: String,
    pub max_output_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageV1 {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_microunits: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelStreamResultV1 {
    #[serde(default)]
    pub selected_route: String,
    pub served_route: String,
    pub content: String,
    pub finish_reason: String,
    pub usage: UsageV1,
    pub correlation_id: String,
    #[serde(default)]
    pub billing: Option<super::billing::BramaBillingSnapshot>,
}

pub trait BramaApiV1 {
    fn health(&self) -> ServiceHealth;
    fn readiness(&self) -> Result<Readiness, brama::BramaError>;
    fn capabilities(&self, meta: &RequestMeta) -> Result<Vec<String>, brama::BramaError>;
    fn catalog(&self, force: bool) -> Result<brama::ModelCatalog, brama::BramaError>;
    fn resolve(
        &self,
        request: &RouteRequest,
        meta: &RequestMeta,
    ) -> Result<brama::ModelEntry, brama::BramaError>;
    fn stream(
        &self,
        request: &ModelRequest,
        meta: &RequestMeta,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ModelStreamResultV1, brama::BramaError>;
}

pub trait WelesApiV1 {
    fn health(&self) -> ServiceHealth;
    fn readiness(&self) -> Result<Readiness, weles::WelesError>;
    fn providers(&self) -> Result<Vec<weles::Provider>, weles::WelesError>;
    fn accounts(&self, provider: Option<&str>) -> Result<Vec<weles::Account>, weles::WelesError>;
    fn begin_login(
        &self,
        provider: &str,
        consumer: &str,
        meta: &RequestMeta,
    ) -> Result<weles::OperationV1, weles::WelesError>;
    fn poll_operation(
        &self,
        operation_id: &str,
        cursor: Option<&str>,
        meta: &RequestMeta,
    ) -> Result<weles::OperationV1, weles::WelesError>;
    fn submit_input(
        &self,
        operation_id: &str,
        field: &str,
        value: &str,
        meta: &RequestMeta,
    ) -> Result<(), weles::WelesError>;
    fn cancel_operation(
        &self,
        operation_id: &str,
        meta: &RequestMeta,
    ) -> Result<(), weles::WelesError>;
    fn refresh(
        &self,
        account: &str,
        meta: &RequestMeta,
    ) -> Result<weles::OperationV1, weles::WelesError>;
    fn logout(
        &self,
        account: &str,
        meta: &RequestMeta,
    ) -> Result<weles::OperationV1, weles::WelesError>;
}

pub trait WelesApiV2 {
    fn billing_status(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::AccountStatus, weles::WelesError>;
    fn payment_methods(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<Vec<super::billing::PaymentMethodReference>, weles::WelesError>;
    fn begin_payment_method_setup(
        &self,
        request: &super::billing::PaymentMethodSetupRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::HostedPaymentSetup, weles::WelesError>;
    fn revoke_payment_method(
        &self,
        account_id: &str,
        payment_method: &super::billing::PaymentMethodReference,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, weles::WelesError>;
    fn purchase_policy(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::PurchasePolicy, weles::WelesError>;
    fn set_purchase_policy(
        &self,
        account_id: &str,
        policy: &super::billing::PurchasePolicy,
        meta: &RequestMeta,
    ) -> Result<super::billing::PurchasePolicy, weles::WelesError>;
    fn disable_purchase_policy(
        &self,
        account_id: &str,
        policy_revision: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::PurchasePolicy, weles::WelesError>;
    fn subscriptions(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<Vec<super::billing::SubscriptionV2>, weles::WelesError>;
    fn quota(
        &self,
        subscription_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::QuotaSnapshot, weles::WelesError>;
    fn quote(
        &self,
        request: &super::billing::QuoteRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::Quote, weles::WelesError>;
    fn purchase(
        &self,
        request: &super::billing::PurchaseRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, weles::WelesError>;
    fn renew(
        &self,
        subscription_id: &str,
        request: &super::billing::RenewRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, weles::WelesError>;
    fn cancel_subscription(
        &self,
        subscription_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, weles::WelesError>;
}

pub fn negotiate(service_min: u32, service_max: u32) -> Result<u32, ContractError> {
    let low = service_min.max(MIN_SCHEMA_VERSION);
    let high = service_max.min(MAX_SCHEMA_VERSION);
    (low <= high)
        .then_some(high)
        .ok_or(ContractError::SchemaSkew {
            service_min,
            service_max,
        })
}

pub fn negotiate_response(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<u32, ContractError> {
    let service_min = headers
        .get("x-jeden-schema-min")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| ContractError::Malformed("invalid x-jeden-schema-min".into()))
        })
        .transpose()?
        .unwrap_or(MIN_SCHEMA_VERSION);
    let service_max = headers
        .get("x-jeden-schema-max")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| ContractError::Malformed("invalid x-jeden-schema-max".into()))
        })
        .transpose()?
        .unwrap_or(MAX_SCHEMA_VERSION);
    if service_min > service_max {
        return Err(ContractError::Malformed(
            "service schema range is inverted".into(),
        ));
    }
    negotiate(service_min, service_max)
}
