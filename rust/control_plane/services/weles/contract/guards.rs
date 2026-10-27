//! What is checked before a billing request is sent, and after its answer
//! arrives.
//!
//! No request field is refused by its name. Every billing request body is the
//! serialization of a closed request type (`control_plane::billing`:
//! `deny_unknown_fields`, no open map or `Value` field) or a literal with the
//! one field the operation takes, so a raw payment detail has no field to
//! travel in; the hosted setup keeps card data at Weles. A name list
//! (`pan`, `cvv`, `cardholder`, …) refused a spelling and let every other
//! one through.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::super::{WelesClient, WelesError};
use crate::control_plane::contract::RequestMeta;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub(super) fn decode_v2<T: DeserializeOwned>(value: Value) -> Result<T, WelesError> {
    serde_json::from_value(value).map_err(|error| {
        WelesError::InvalidResponse(format!(
            "response did not match the bounded Weles v2 contract: {error}"
        ))
    })
}

pub(super) fn validate_identifier(value: &str) -> Result<(), WelesError> {
    if value.is_empty() || value.chars().any(char::is_control) {
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
