//! The real backend: the same billing commands carried out against the
//! platform billing service.

use super::model::{
    BillingBackend, BillingError, BillingPolicy, MutationRequest, PaymentMethodSetup,
    PurchaseRequest, SubscriptionMutationResult, SubscriptionStatus, SubscriptionSummary,
};
use crate::control_plane::billing::{
    HostedPaymentSetup, PaymentMethodSetupRequest, QuoteRequest, RenewRequest as WelesRenewRequest,
};
use crate::control_plane::contract::{RequestMeta, WelesApiV2};
use crate::control_plane::weles::WelesClient;

mod convert;

use convert::{backend, correlation, operation, policy_from_weles, policy_to_weles, summary};

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
