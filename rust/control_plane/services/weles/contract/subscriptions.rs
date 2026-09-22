//! Reading a subscription's quota and pricing a purchase, and refusing an
//! answer that names another subscription or account than the one asked for.
//!
//! Split out of `contract/billing.rs`, which formatting had pushed past the
//! module line cap.

use super::super::{encode_path_segment, WelesClient, WelesError};
use super::guards::read_v2;
use crate::control_plane::billing;
use crate::control_plane::contract::RequestMeta;

pub(super) fn quota(
    client: &WelesClient,
    subscription_id: &str,
    meta: &RequestMeta,
) -> Result<billing::QuotaSnapshot, WelesError> {
    let quota: billing::QuotaSnapshot = read_v2(
        client,
        format!(
            "/subscriptions/{}/quota",
            encode_path_segment(subscription_id)
        ),
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

pub(super) fn quote(
    client: &WelesClient,
    request: &billing::QuoteRequest,
    meta: &RequestMeta,
) -> Result<billing::Quote, WelesError> {
    let body = serde_json::to_value(request)
        .map_err(|_| WelesError::InvalidRequest("quote request encoding failed"))?;
    let quote: billing::Quote = client.request_v2(
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
