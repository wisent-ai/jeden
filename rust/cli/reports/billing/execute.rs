//! Running a parsed billing command, and refusing to spend money without a
//! fresh, explicit approval.
//!
//! Split out of `cli/reports/billing.rs`, which had grown past the module line
//! cap.

use super::model::{BillingBackend, BillingCommand, BillingError, BillingPolicy};
use serde::Serialize;

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
