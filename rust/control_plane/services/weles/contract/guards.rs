//! What is checked before a billing request is sent, and after its answer
//! arrives.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::super::{WelesClient, WelesError};
use crate::control_plane::contract::RequestMeta;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub(crate) fn reject_forbidden_payment_fields(value: &Value) -> Result<(), WelesError> {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if matches!(
                    normalized.as_str(),
                    "pan"
                        | "cardnumber"
                        | "cvv"
                        | "cvc"
                        | "processortoken"
                        | "rawpaymentdetails"
                        | "billingaddress"
                        | "shippingaddress"
                        | "fulladdress"
                        | "cardholder"
                        | "expiry"
                        | "expiration"
                ) {
                    return Err(WelesError::InvalidRequest(
                        "raw payment fields are forbidden",
                    ));
                }
                reject_forbidden_payment_fields(child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_forbidden_payment_fields(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn decode_v2<T: DeserializeOwned>(value: Value) -> Result<T, WelesError> {
    serde_json::from_value(value).map_err(|_| {
        WelesError::InvalidResponse("response did not match the bounded Weles v2 contract".into())
    })
}

pub(super) fn validate_identifier(value: &str) -> Result<(), WelesError> {
    if value.is_empty()
        || value.len() > super::billing::MAX_BILLING_STRING_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(WelesError::InvalidResponse(
            "billing identifier is invalid".into(),
        ));
    }
    Ok(())
}

/// Every plain read goes through one place, so no call can quietly skip the
/// method, the encoding or the decoding the others use.
pub(super) fn read_v2<T: DeserializeOwned>(
    client: &WelesClient,
    path: String,
    meta: &RequestMeta,
) -> Result<T, WelesError> {
    client.request_v2(reqwest::Method::GET, &path, None, meta, false)
}
