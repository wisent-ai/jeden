use serde::Serialize;
use std::collections::BTreeMap;

use crate::control_plane::billing::{
    HostedPaymentSetup, PaymentMethodSetupRequest, PolicyPeriod, PurchasePolicy, QuoteRequest,
    RenewRequest as WelesRenewRequest, SubscriptionState,
};
use crate::control_plane::contract::{RequestMeta, WelesApiV2};
use crate::control_plane::weles::WelesClient;

const MAX_POLICY_CAP_MICROUNITS: u64 = 1_000_000_000_000_000;
const MAX_POLICY_ITEMS: usize = 128;

pub(crate) const BILLING_SLASH_HANDLERS: [(&str, &str); 9] = [
    ("/payment-method setup", "payment-method.setup"),
    ("/billing policy get", "billing.policy.get"),
    ("/billing policy set", "billing.policy.set"),
    ("/billing policy reset", "billing.policy.reset"),
    ("/subscriptions list", "subscriptions.list"),
    ("/subscriptions status", "subscriptions.status"),
    ("/subscriptions disable", "subscriptions.disable"),
    ("/subscriptions purchase", "subscriptions.purchase"),
    ("/subscriptions renew", "subscriptions.renew"),
];

pub(crate) fn billing_slash_handlers() -> &'static [(&'static str, &'static str)] {
    &BILLING_SLASH_HANDLERS
}

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

fn flag_map(tokens: &[&str]) -> Result<BTreeMap<String, Option<String>>, BillingError> {
    let mut result = BTreeMap::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if !token.starts_with("--") {
            return Err(BillingError::InvalidCommand(format!(
                "unexpected argument `{token}`"
            )));
        }
        let key = token.trim_start_matches("--");
        if key.is_empty() || result.contains_key(key) {
            return Err(BillingError::InvalidCommand(format!(
                "invalid or duplicate option `{token}`"
            )));
        }
        let boolean = matches!(key, "approve" | "enabled" | "auto-renew");
        if boolean {
            result.insert(key.into(), None);
            index += 1;
            continue;
        }
        let value = tokens
            .get(index + 1)
            .filter(|next| !next.starts_with("--"))
            .ok_or_else(|| {
                BillingError::InvalidCommand(format!("option `{token}` requires a value"))
            })?;
        result.insert(key.into(), Some((*value).into()));
        index += 2;
    }
    Ok(result)
}
fn required(flags: &BTreeMap<String, Option<String>>, key: &str) -> Result<String, BillingError> {
    flags
        .get(key)
        .and_then(Clone::clone)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BillingError::InvalidCommand(format!("--{key} is required")))
}
fn only(flags: &BTreeMap<String, Option<String>>, allowed: &[&str]) -> Result<(), BillingError> {
    if let Some(key) = flags.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(BillingError::InvalidCommand(format!(
            "unknown option `--{key}`"
        )))
    } else {
        Ok(())
    }
}
fn parse_u64(flags: &BTreeMap<String, Option<String>>, key: &str) -> Result<u64, BillingError> {
    required(flags, key)?
        .parse()
        .map_err(|_| BillingError::InvalidCommand(format!("--{key} must be an unsigned integer")))
}
fn csv(flags: &BTreeMap<String, Option<String>>, key: &str) -> Result<Vec<String>, BillingError> {
    let values = required(flags, key)?
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() || values.len() > MAX_POLICY_ITEMS {
        Err(BillingError::InvalidCommand(format!(
            "--{key} must contain 1..={MAX_POLICY_ITEMS} values"
        )))
    } else {
        Ok(values)
    }
}

pub(crate) fn parse_billing_command(input: &str) -> Result<BillingCommand, BillingError> {
    let tokens = input.split_whitespace().collect::<Vec<_>>();
    let (prefix, option_start) = if tokens.starts_with(&["/payment-method", "setup"]) {
        ("payment-setup", 2)
    } else if tokens.starts_with(&["/billing", "policy", "get"]) {
        ("policy-get", 3)
    } else if tokens.starts_with(&["/billing", "policy", "set"]) {
        ("policy-set", 3)
    } else if tokens.starts_with(&["/billing", "policy", "reset"]) {
        ("policy-reset", 3)
    } else if tokens.starts_with(&["/subscriptions", "list"]) {
        ("subscriptions-list", 2)
    } else if tokens.starts_with(&["/subscriptions", "status"]) {
        ("subscription-status", 2)
    } else if tokens.starts_with(&["/subscriptions", "disable"]) {
        ("subscription-disable", 2)
    } else if tokens.starts_with(&["/subscriptions", "purchase"]) {
        ("subscription-purchase", 2)
    } else if tokens.starts_with(&["/subscriptions", "renew"]) {
        ("subscription-renew", 2)
    } else {
        return Err(BillingError::InvalidCommand(
            "unknown billing command".into(),
        ));
    };
    let flags = flag_map(&tokens[option_start..])?;
    let account_id = required(&flags, "account")?;
    match prefix {
        "payment-setup" => {
            only(&flags, &["account"])?;
            Ok(BillingCommand::PaymentMethodSetup { account_id })
        }
        "policy-get" => {
            only(&flags, &["account"])?;
            Ok(BillingCommand::PolicyGet { account_id })
        }
        "policy-set" => {
            only(
                &flags,
                &[
                    "account",
                    "enabled",
                    "auto-renew",
                    "products",
                    "currencies",
                    "max-single",
                    "max-period",
                    "period",
                    "revision",
                    "valid-until",
                    "approve",
                ],
            )?;
            let policy = BillingPolicy::new(
                flags.contains_key("enabled"),
                flags.contains_key("auto-renew"),
                csv(&flags, "products")?,
                csv(&flags, "currencies")?,
                parse_u64(&flags, "max-single")?,
                parse_u64(&flags, "max-period")?,
                flags
                    .get("period")
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| "month".into()),
                required(&flags, "revision")?,
                parse_u64(&flags, "valid-until")?,
            );
            validate_policy(&policy)?;
            Ok(BillingCommand::PolicySet {
                account_id,
                policy,
                approval: PolicyApproval::new(flags.contains_key("approve")),
            })
        }
        "policy-reset" => {
            only(&flags, &["account", "approve"])?;
            Ok(BillingCommand::PolicyReset {
                account_id,
                approval: PolicyApproval::new(flags.contains_key("approve")),
            })
        }
        "subscriptions-list" => {
            only(&flags, &["account"])?;
            Ok(BillingCommand::SubscriptionsList { account_id })
        }
        "subscription-status" => {
            only(&flags, &["account", "subscription"])?;
            Ok(BillingCommand::SubscriptionStatus {
                account_id,
                subscription_id: required(&flags, "subscription")?,
            })
        }
        "subscription-disable" => {
            only(
                &flags,
                &["account", "subscription", "idempotency", "approve"],
            )?;
            Ok(BillingCommand::SubscriptionDisable {
                account_id,
                subscription_id: required(&flags, "subscription")?,
                request: MutationRequest::new(
                    required(&flags, "idempotency")?,
                    flags.contains_key("approve"),
                ),
            })
        }
        "subscription-purchase" => {
            only(
                &flags,
                &["account", "product", "currency", "idempotency", "approve"],
            )?;
            Ok(BillingCommand::SubscriptionPurchase {
                account_id,
                request: PurchaseRequest::new(
                    required(&flags, "product")?,
                    required(&flags, "currency")?,
                    required(&flags, "idempotency")?,
                    flags.contains_key("approve"),
                ),
            })
        }
        "subscription-renew" => {
            only(
                &flags,
                &["account", "subscription", "idempotency", "approve"],
            )?;
            Ok(BillingCommand::SubscriptionRenew {
                account_id,
                subscription_id: required(&flags, "subscription")?,
                request: MutationRequest::new(
                    required(&flags, "idempotency")?,
                    flags.contains_key("approve"),
                ),
            })
        }
        _ => unreachable!(),
    }
}

fn validate_policy(policy: &BillingPolicy) -> Result<(), BillingError> {
    if policy.auto_renew && !policy.enabled {
        return Err(BillingError::InvalidCommand(
            "--auto-renew requires --enabled".into(),
        ));
    }
    if policy.max_single_microunits == 0
        || policy.max_single_microunits > policy.max_period_microunits
        || policy.max_period_microunits > MAX_POLICY_CAP_MICROUNITS
    {
        return Err(BillingError::InvalidCommand(format!(
            "policy caps must satisfy 0 < max-single <= max-period <= {MAX_POLICY_CAP_MICROUNITS}"
        )));
    }
    if !matches!(policy.period.as_str(), "day" | "month" | "billing-cycle") {
        return Err(BillingError::InvalidCommand(
            "--period must be day, month, or billing-cycle".into(),
        ));
    }
    if policy.revision.trim().is_empty() || policy.valid_until_ms == 0 {
        return Err(BillingError::InvalidCommand(
            "policy requires a pinned revision and bounded validity".into(),
        ));
    }
    Ok(())
}

fn approval_summary(account_id: &str, policy: &BillingPolicy) -> String {
    format!("Approval required: account={account_id}; auto-purchase={}; auto-renew={}; products={}; currencies={}; max-single={} microunits; max-period={} microunits/{}; revision={}; valid-until={}. Re-run with --approve.", policy.enabled, policy.auto_renew, policy.allowed_products.join(","), policy.allowed_currencies.join(","), policy.max_single_microunits, policy.max_period_microunits, policy.period, policy.revision, policy.valid_until_ms)
}
fn mutation_approval(
    action: &str,
    account_id: &str,
    subscription_id: Option<&str>,
    approved: bool,
) -> Result<(), BillingError> {
    if approved {
        Ok(())
    } else {
        Err(BillingError::ApprovalRequired(format!("Approval required: action={action}; account={account_id}; subscription={}. Re-run with --approve.", subscription_id.unwrap_or("new"))))
    }
}

fn json<T: Serialize>(value: &T) -> Result<String, BillingError> {
    serde_json::to_string_pretty(value)
        .map(|value| value + "\n")
        .map_err(|_| BillingError::Backend("failed to render redacted billing output".into()))
}

pub(crate) fn execute_billing_command(
    backend: &dyn BillingBackend,
    command: BillingCommand,
    json_output: bool,
) -> Result<String, BillingError> {
    match command {
        BillingCommand::PaymentMethodSetup { account_id } => {
            let setup = backend.payment_method_setup(&account_id)?;
            if json_output {
                json(&setup)
            } else {
                Ok(format!("{}\n", setup.hosted_url))
            }
        }
        BillingCommand::PolicyGet { account_id } => {
            let policy = backend.policy_get(&account_id)?;
            if json_output {
                json(&policy)
            } else {
                Ok(format!("Account {account_id}: auto-purchase={}, auto-renew={}, products={}, currencies={}, max-single={}, max-period={}/{}, revision={}, valid-until={}\n", policy.enabled, policy.auto_renew, policy.allowed_products.join(","), policy.allowed_currencies.join(","), policy.max_single_microunits, policy.max_period_microunits, policy.period, policy.revision, policy.valid_until_ms))
            }
        }
        BillingCommand::PolicySet {
            account_id,
            policy,
            approval,
        } => {
            if !approval.approved {
                return Err(BillingError::ApprovalRequired(approval_summary(
                    &account_id,
                    &policy,
                )));
            }
            let applied = backend.policy_set(&account_id, policy, approval)?;
            if json_output {
                json(&applied)
            } else {
                Ok(format!(
                    "Applied billing policy {} to account {}.\n",
                    applied.revision, account_id
                ))
            }
        }
        BillingCommand::PolicyReset {
            account_id,
            approval,
        } => {
            mutation_approval("policy-reset", &account_id, None, approval.approved)?;
            let policy = backend.policy_reset(&account_id)?;
            if json_output {
                json(&policy)
            } else {
                Ok(format!(
                    "Disabled automatic billing policy {} for account {}.\n",
                    policy.revision, account_id
                ))
            }
        }
        BillingCommand::SubscriptionsList { account_id } => {
            let subscriptions = backend.subscriptions_list(&account_id)?;
            if json_output {
                json(&subscriptions)
            } else if subscriptions.is_empty() {
                Ok(format!("No subscriptions for account {account_id}.\n"))
            } else {
                Ok(subscriptions
                    .into_iter()
                    .map(|item| {
                        format!(
                            "{} · account={} · provider={} · product={} · status={}\n",
                            item.id,
                            item.account_id,
                            item.provider_id,
                            item.product_id,
                            item.status
                        )
                    })
                    .collect())
            }
        }
        BillingCommand::SubscriptionStatus {
            account_id,
            subscription_id,
        } => {
            let status = backend.subscription_status(&account_id, &subscription_id)?;
            if json_output {
                json(&status)
            } else {
                Ok(format!("Subscription {} · account={} · provider={} · product={} · status={} · quota-revision={}\n{}", status.subscription.id, status.subscription.account_id, status.subscription.provider_id, status.subscription.product_id, status.subscription.status, status.quota_revision, status.quota.into_iter().map(|bucket| match (bucket.remaining, bucket.limit) {
                    (Some(remaining), Some(limit)) => format!("  {}: {remaining}/{limit} remaining ({:?})\n", bucket.bucket_id, bucket.state),
                    _ => format!("  {}: {:?}\n", bucket.bucket_id, bucket.state),
                }).collect::<String>()))
            }
        }
        BillingCommand::SubscriptionDisable {
            account_id,
            subscription_id,
            request,
        } => {
            mutation_approval(
                "subscription-disable",
                &account_id,
                Some(&subscription_id),
                request.approved,
            )?;
            let result = backend.subscription_disable(&account_id, &subscription_id, request)?;
            if json_output {
                json(&result)
            } else {
                Ok(format!(
                    "Subscription {subscription_id} disable operation {}: {}.\n",
                    result.operation_id, result.state
                ))
            }
        }
        BillingCommand::SubscriptionPurchase {
            account_id,
            request,
        } => {
            mutation_approval(
                "subscription-purchase",
                &account_id,
                None,
                request.mutation.approved,
            )?;
            let result = backend.subscription_purchase(&account_id, request)?;
            if json_output {
                json(&result)
            } else {
                Ok(format!(
                    "Subscription purchase operation {}: {}.\n",
                    result.operation_id, result.state
                ))
            }
        }
        BillingCommand::SubscriptionRenew {
            account_id,
            subscription_id,
            request,
        } => {
            mutation_approval(
                "subscription-renew",
                &account_id,
                Some(&subscription_id),
                request.approved,
            )?;
            let result = backend.subscription_renew(&account_id, &subscription_id, request)?;
            if json_output {
                json(&result)
            } else {
                Ok(format!(
                    "Subscription {subscription_id} renewal operation {}: {}.\n",
                    result.operation_id, result.state
                ))
            }
        }
    }
}

pub(crate) struct WelesBillingBackend {
    client: WelesClient,
}
impl WelesBillingBackend {
    pub(crate) fn from_env() -> Self {
        Self {
            client: WelesClient::from_env(),
        }
    }
}

fn correlation(action: &str, identity: &str) -> String {
    format!("jeden-{action}-{identity}")
}
fn policy_from_weles(policy: PurchasePolicy) -> BillingPolicy {
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
fn policy_to_weles(policy: BillingPolicy) -> PurchasePolicy {
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
fn state_text(state: SubscriptionState) -> &'static str {
    match state {
        SubscriptionState::Active => "active",
        SubscriptionState::PastDue => "past_due",
        SubscriptionState::Paused => "paused",
        SubscriptionState::Cancelled => "cancelled",
        SubscriptionState::Expired => "expired",
    }
}
fn summary(subscription: crate::control_plane::billing::SubscriptionV2) -> SubscriptionSummary {
    SubscriptionSummary::new(
        subscription.id,
        subscription.account_id,
        subscription.provider_id,
        subscription.product_id,
        state_text(subscription.status),
        subscription.renews_at_ms,
    )
}
fn operation(result: crate::control_plane::billing::OperationResult) -> SubscriptionMutationResult {
    SubscriptionMutationResult::new(
        result.operation_id,
        format!("{:?}", result.state).to_ascii_lowercase(),
    )
}
fn backend(error: crate::control_plane::weles::WelesError) -> BillingError {
    BillingError::Backend(error.to_string())
}

impl BillingBackend for WelesBillingBackend {
    fn payment_method_setup(&self, account_id: &str) -> Result<PaymentMethodSetup, BillingError> {
        let return_url = std::env::var("WELES_PAYMENT_RETURN_URL").map_err(|_| {
            BillingError::Backend(
                "WELES_PAYMENT_RETURN_URL must be configured to an HTTPS URL".into(),
            )
        })?;
        let request = PaymentMethodSetupRequest {
            account_id: account_id.into(),
            return_url,
        };
        let setup: HostedPaymentSetup = self
            .client
            .begin_payment_method_setup(
                &request,
                &RequestMeta::mutation_v2(
                    correlation("payment-setup", account_id),
                    correlation("payment-setup", account_id),
                ),
            )
            .map_err(backend)?;
        Ok(PaymentMethodSetup::new(setup.hosted_url))
    }
    fn policy_get(&self, account_id: &str) -> Result<BillingPolicy, BillingError> {
        self.client
            .purchase_policy(
                account_id,
                &RequestMeta::read_v2(correlation("policy-read", account_id)),
            )
            .map(policy_from_weles)
            .map_err(backend)
    }
    fn policy_set(
        &self,
        account_id: &str,
        policy: BillingPolicy,
        _approval: PolicyApproval,
    ) -> Result<BillingPolicy, BillingError> {
        let policy = policy_to_weles(policy);
        self.client
            .set_purchase_policy(
                account_id,
                &policy,
                &RequestMeta::mutation_v2(
                    correlation("policy-set", account_id),
                    format!("policy-set-{}-{}", account_id, policy.revision),
                ),
            )
            .map(policy_from_weles)
            .map_err(backend)
    }
    fn policy_reset(&self, account_id: &str) -> Result<BillingPolicy, BillingError> {
        let current = self
            .client
            .purchase_policy(
                account_id,
                &RequestMeta::read_v2(correlation("policy-read", account_id)),
            )
            .map_err(backend)?;
        self.client
            .disable_purchase_policy(
                account_id,
                &current.revision,
                &RequestMeta::mutation_v2(
                    correlation("policy-reset", account_id),
                    format!("policy-reset-{}-{}", account_id, current.revision),
                ),
            )
            .map(policy_from_weles)
            .map_err(backend)
    }
    fn subscriptions_list(
        &self,
        account_id: &str,
    ) -> Result<Vec<SubscriptionSummary>, BillingError> {
        self.client
            .subscriptions(
                account_id,
                &RequestMeta::read_v2(correlation("subscriptions", account_id)),
            )
            .map(|items| items.into_iter().map(summary).collect())
            .map_err(backend)
    }
    fn subscription_status(
        &self,
        account_id: &str,
        subscription_id: &str,
    ) -> Result<SubscriptionStatus, BillingError> {
        let subscription = self
            .client
            .subscriptions(
                account_id,
                &RequestMeta::read_v2(correlation("subscriptions", account_id)),
            )
            .map_err(backend)?
            .into_iter()
            .find(|item| item.id == subscription_id)
            .ok_or_else(|| {
                BillingError::NotFound(format!(
                    "subscription `{subscription_id}` does not belong to account `{account_id}`"
                ))
            })?;
        let quota = self
            .client
            .quota(
                subscription_id,
                &RequestMeta::read_v2(correlation("quota", subscription_id)),
            )
            .map_err(backend)?;
        Ok(SubscriptionStatus::new(
            summary(subscription),
            quota.revision,
            quota.observed_at_ms,
            quota
                .buckets
                .into_iter()
                .map(|bucket| QuotaSummary {
                    bucket_id: bucket.bucket_id,
                    state: bucket.state,
                    limit: bucket.limit,
                    remaining: bucket.remaining,
                    resets_at_ms: bucket.resets_at_ms,
                })
                .collect(),
        ))
    }
    fn subscription_disable(
        &self,
        account_id: &str,
        subscription_id: &str,
        request: MutationRequest,
    ) -> Result<SubscriptionMutationResult, BillingError> {
        self.subscription_status(account_id, subscription_id)?;
        self.client
            .cancel_subscription(
                subscription_id,
                &RequestMeta::mutation_v2(
                    correlation("subscription-disable", subscription_id),
                    request.idempotency_key,
                ),
            )
            .map(operation)
            .map_err(backend)
    }
    fn subscription_purchase(
        &self,
        account_id: &str,
        request: PurchaseRequest,
    ) -> Result<SubscriptionMutationResult, BillingError> {
        let status = self
            .client
            .billing_status(
                account_id,
                &RequestMeta::read_v2(correlation("billing-status", account_id)),
            )
            .map_err(backend)?;
        let quote = self
            .client
            .quote(
                &QuoteRequest {
                    account_id: account_id.into(),
                    provider_id: status.provider_id,
                    product_id: request.product_id,
                    currency: request.currency,
                },
                &RequestMeta::read_v2(correlation("quote", account_id)),
            )
            .map_err(backend)?;
        let policy = self
            .client
            .purchase_policy(
                account_id,
                &RequestMeta::read_v2(correlation("policy-read", account_id)),
            )
            .map_err(backend)?;
        let methods = self
            .client
            .payment_methods(
                account_id,
                &RequestMeta::read_v2(correlation("payment-methods", account_id)),
            )
            .map_err(backend)?;
        let payment_method_reference = match methods.as_slice() {
            [method] => method.clone(),
            [] => {
                return Err(BillingError::NotFound(format!(
                    "account `{account_id}` has no payment method; run /payment-method setup"
                )))
            }
            _ => {
                return Err(BillingError::Backend(format!(
                    "account `{account_id}` has multiple payment methods; choose one in Weles"
                )))
            }
        };
        let purchase = crate::control_plane::billing::PurchaseRequest {
            quote_id: quote.id,
            quote_revision: quote.revision,
            policy_revision: policy.revision,
            payment_method_reference,
        };
        self.client
            .purchase(
                &purchase,
                &RequestMeta::mutation_v2(
                    correlation("subscription-purchase", account_id),
                    request.mutation.idempotency_key,
                ),
            )
            .map(operation)
            .map_err(backend)
    }
    fn subscription_renew(
        &self,
        account_id: &str,
        subscription_id: &str,
        request: MutationRequest,
    ) -> Result<SubscriptionMutationResult, BillingError> {
        let subscription = self
            .client
            .subscriptions(
                account_id,
                &RequestMeta::read_v2(correlation("subscriptions", account_id)),
            )
            .map_err(backend)?
            .into_iter()
            .find(|item| item.id == subscription_id)
            .ok_or_else(|| {
                BillingError::NotFound(format!(
                    "subscription `{subscription_id}` does not belong to account `{account_id}`"
                ))
            })?;
        let policy = self
            .client
            .purchase_policy(
                account_id,
                &RequestMeta::read_v2(correlation("policy-read", account_id)),
            )
            .map_err(backend)?;
        let currency = policy.allowed_currencies.first().cloned().ok_or_else(|| {
            BillingError::Backend("billing policy has no allowed currency".into())
        })?;
        let quote = self
            .client
            .quote(
                &QuoteRequest {
                    account_id: account_id.into(),
                    provider_id: subscription.provider_id,
                    product_id: subscription.product_id,
                    currency,
                },
                &RequestMeta::read_v2(correlation("renew-quote", subscription_id)),
            )
            .map_err(backend)?;
        let renew = WelesRenewRequest {
            quote_id: quote.id,
            quote_revision: quote.revision,
            policy_revision: policy.revision,
        };
        self.client
            .renew(
                subscription_id,
                &renew,
                &RequestMeta::mutation_v2(
                    correlation("subscription-renew", subscription_id),
                    request.idempotency_key,
                ),
            )
            .map(operation)
            .map_err(backend)
    }
}

pub(crate) fn handle_billing_slash(input: &str, json_output: bool) -> Result<String, String> {
    let command = parse_billing_command(input).map_err(|error| error.to_string())?;
    execute_billing_command(&WelesBillingBackend::from_env(), command, json_output)
        .map_err(|error| error.to_string())
}
