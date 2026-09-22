//! Reading a billing command the operator typed, and refusing it rather than
//! guessing.
//!
//! Split out of `cli/reports/billing.rs`, which had grown past the module line
//! cap.

use super::model::{
    BillingCommand, BillingError, BillingPolicy, MutationRequest, PolicyApproval, PurchaseRequest,
};
use std::collections::BTreeMap;

const MAX_POLICY_CAP_MICROUNITS: u64 = 1_000_000_000_000_000;
const MAX_POLICY_ITEMS: usize = 128;

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

pub(super) fn validate_policy(policy: &BillingPolicy) -> Result<(), BillingError> {
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
