//! The two opaque references this product carries on behalf of a payment
//! provider, held as their own types so they cannot be printed by accident.
//!
//! Split out of `control_plane/billing.rs`, which had grown past the module
//! line cap.

use super::MAX_BILLING_STRING_BYTES;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

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
