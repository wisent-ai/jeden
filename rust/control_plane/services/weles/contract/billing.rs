//! The second version of the billing contract: status, subscriptions, quotas
//! and hosted payment setup.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use crate::control_plane::billing;
use crate::control_plane::contract::RequestMeta;
use super::guards::{decode_v2, read_v2, reject_forbidden_payment_fields, validate_identifier};
use super::super::{encode_path_segment, WelesClient, WelesError};
use serde_json::{json, Value};

impl crate::control_plane::contract::WelesApiV2 for WelesClient {
    fn billing_status(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<billing::AccountStatus, WelesError> {
        let status: billing::AccountStatus = read_v2(
            self,
            format!("/accounts/{}/billing/status", encode_path_segment(account_id)),
            meta,
        )?;
        validate_identifier(&status.account_id)?;
        validate_identifier(&status.provider_id)?;
        if status.account_id != account_id
            || status.capabilities.len() > billing::MAX_BILLING_ITEMS
        {
            return Err(WelesError::InvalidResponse(
                "billing status identity or capability count is invalid".into(),
            ));
        }
        Ok(status)
    }

    fn payment_methods(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<Vec<billing::PaymentMethodReference>, WelesError> {
        let value: Value = read_v2(
            self,
            format!("/accounts/{}/payment-methods", encode_path_segment(account_id)),
            meta,
        )?;
        let methods: Vec<billing::PaymentMethodReference> =
            decode_v2(value.get("paymentMethods").cloned().unwrap_or(value))?;
        if methods.len() > billing::MAX_BILLING_ITEMS {
            return Err(WelesError::InvalidResponse(
                "payment method count exceeds limit".into(),
            ));
        }
        Ok(methods)
    }

    fn begin_payment_method_setup(
        &self,
        request: &billing::PaymentMethodSetupRequest,
        meta: &RequestMeta,
    ) -> Result<billing::HostedPaymentSetup, WelesError> {
        validate_identifier(&request.account_id)
            .map_err(|_| WelesError::InvalidRequest("account id is invalid"))?;
        let return_url = url::Url::parse(&request.return_url)
            .map_err(|_| WelesError::InvalidRequest("return URL is invalid"))?;
        if return_url.scheme() != "https" {
            return Err(WelesError::InvalidRequest("return URL must use HTTPS"));
        }
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("setup request encoding failed"))?;
        let setup = self.request_v2(
            reqwest::Method::POST,
            "/payment-method-setups",
            Some(&body),
            meta,
            true,
        )?;
        self.validate_hosted_setup(&setup)?;
        Ok(setup)
    }

    fn revoke_payment_method(
        &self,
        account_id: &str,
        payment_method: &billing::PaymentMethodReference,
        meta: &RequestMeta,
    ) -> Result<billing::OperationResult, WelesError> {
        self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/accounts/{}/payment-methods/{}/revoke",
                encode_path_segment(account_id),
                encode_path_segment(payment_method.as_str())
            ),
            Some(&json!({})),
            meta,
            true,
        )
    }

    fn purchase_policy(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<billing::PurchasePolicy, WelesError> {
        let policy = read_v2(
            self,
            format!("/accounts/{}/purchase-policy", encode_path_segment(account_id)),
            meta,
        )?;
        billing::validate_policy(&policy)
            .map_err(|message| WelesError::InvalidResponse(message.into()))?;
        Ok(policy)
    }

    fn set_purchase_policy(
        &self,
        account_id: &str,
        policy: &billing::PurchasePolicy,
        meta: &RequestMeta,
    ) -> Result<billing::PurchasePolicy, WelesError> {
        billing::validate_policy(policy).map_err(WelesError::InvalidRequest)?;
        let body = serde_json::to_value(policy)
            .map_err(|_| WelesError::InvalidRequest("policy encoding failed"))?;
        let applied = self.request_v2(
            reqwest::Method::PUT,
            &format!(
                "/accounts/{}/purchase-policy",
                encode_path_segment(account_id)
            ),
            Some(&body),
            meta,
            true,
        )?;
        billing::validate_policy(&applied)
            .map_err(|message| WelesError::InvalidResponse(message.into()))?;
        Ok(applied)
    }

    fn disable_purchase_policy(
        &self,
        account_id: &str,
        policy_revision: &str,
        meta: &RequestMeta,
    ) -> Result<billing::PurchasePolicy, WelesError> {
        validate_identifier(policy_revision)
            .map_err(|_| WelesError::InvalidRequest("policy revision is invalid"))?;
        let body = json!({"policyRevision": policy_revision});
        let policy = self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/accounts/{}/purchase-policy/disable",
                encode_path_segment(account_id)
            ),
            Some(&body),
            meta,
            true,
        )?;
        billing::validate_policy(&policy)
            .map_err(|message| WelesError::InvalidResponse(message.into()))?;
        if policy.enabled || policy.auto_renew {
            return Err(WelesError::InvalidResponse(
                "disabled policy remains enabled".into(),
            ));
        }
        Ok(policy)
    }

    fn subscriptions(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<Vec<billing::SubscriptionV2>, WelesError> {
        let value: Value = read_v2(
            self,
            format!("/accounts/{}/subscriptions", encode_path_segment(account_id)),
            meta,
        )?;
        let subscriptions: Vec<billing::SubscriptionV2> =
            decode_v2(value.get("subscriptions").cloned().unwrap_or(value))?;
        if subscriptions.len() > billing::MAX_BILLING_ITEMS
            || subscriptions
                .iter()
                .any(|subscription| subscription.account_id != account_id)
        {
            return Err(WelesError::InvalidResponse(
                "subscription count or account identity is invalid".into(),
            ));
        }
        Ok(subscriptions)
    }

    fn quota(
        &self,
        subscription_id: &str,
        meta: &RequestMeta,
    ) -> Result<billing::QuotaSnapshot, WelesError> {
        let quota: billing::QuotaSnapshot = read_v2(
            self,
            format!("/subscriptions/{}/quota", encode_path_segment(subscription_id)),
            meta,
        )?;
        if quota.subscription_id != subscription_id
            || quota.buckets.len() > billing::MAX_BILLING_ITEMS
            || quota.buckets.iter().any(|bucket| {
                matches!(
                    (bucket.remaining, bucket.limit),
                    (Some(remaining), Some(limit)) if remaining > limit
                )
            })
        {
            return Err(WelesError::InvalidResponse(
                "quota identity, count, or remaining amount is invalid".into(),
            ));
        }
        Ok(quota)
    }

    fn quote(
        &self,
        request: &billing::QuoteRequest,
        meta: &RequestMeta,
    ) -> Result<billing::Quote, WelesError> {
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("quote request encoding failed"))?;
        let quote: billing::Quote = self.request_v2(
            reqwest::Method::POST,
            "/subscription-quotes",
            Some(&body),
            meta,
            false,
        )?;
        if quote.account_id != request.account_id
            || quote.provider_id != request.provider_id
            || quote.product_id != request.product_id
            || quote.currency != request.currency
        {
            return Err(WelesError::InvalidResponse(
                "quote identity does not match request".into(),
            ));
        }
        Ok(quote)
    }

    fn purchase(
        &self,
        request: &billing::PurchaseRequest,
        meta: &RequestMeta,
    ) -> Result<billing::OperationResult, WelesError> {
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("purchase request encoding failed"))?;
        self.request_v2(
            reqwest::Method::POST,
            "/subscriptions/purchase",
            Some(&body),
            meta,
            true,
        )
    }

    fn renew(
        &self,
        subscription_id: &str,
        request: &billing::RenewRequest,
        meta: &RequestMeta,
    ) -> Result<billing::OperationResult, WelesError> {
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("renew request encoding failed"))?;
        self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/subscriptions/{}/renew",
                encode_path_segment(subscription_id)
            ),
            Some(&body),
            meta,
            true,
        )
    }

    fn cancel_subscription(
        &self,
        subscription_id: &str,
        meta: &RequestMeta,
    ) -> Result<billing::OperationResult, WelesError> {
        self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/subscriptions/{}/cancel",
                encode_path_segment(subscription_id)
            ),
            Some(&json!({})),
            meta,
            true,
        )
    }
}
