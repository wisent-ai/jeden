//! Reading the model routing block of a configuration, and refusing it in
//! the operator's own words when it is wrong.
//!
//! Split out of `agent/runtime/routing.rs`, which had grown past the module
//! line cap.

use crate::model_router::{RetryPolicy, RouteDescriptor};
use serde_json::Value;

const MAX_CONFIGURED_ROUTES: usize = 16;

pub(super) fn duration_setting(
    value: &Value,
    key: &str,
    default_ms: u64,
) -> Result<std::time::Duration, String> {
    let millis = match value.get(key) {
        None => default_ms,
        Some(raw) => raw
            .as_u64()
            .filter(|millis| *millis > 0)
            .ok_or_else(|| format!("modelRouting.retry.{key} must be a positive integer"))?,
    };
    Ok(std::time::Duration::from_millis(millis))
}

pub(super) fn route_descriptors(
    value: Option<&Value>,
    key: &str,
) -> Result<Vec<RouteDescriptor>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| format!("modelRouting.{key} must be an array"))?;
    if entries.len() > MAX_CONFIGURED_ROUTES {
        return Err(format!(
            "modelRouting.{key} exceeds the {MAX_CONFIGURED_ROUTES}-route limit"
        ));
    }
    let mut routes = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| format!("modelRouting.{key}[{index}] must be an object"))?;
        let model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| {
                format!("modelRouting.{key}[{index}].model must be a non-empty string")
            })?;
        let service_tier = match object.get("serviceTier") {
            None | Some(Value::Null) => None,
            Some(raw) => Some(
                raw.as_str()
                    .map(str::trim)
                    .filter(|tier| !tier.is_empty())
                    .ok_or_else(|| {
                        format!(
                            "modelRouting.{key}[{index}].serviceTier must be a non-empty string"
                        )
                    })?
                    .to_string(),
            ),
        };
        routes.push(RouteDescriptor {
            model: model.to_string(),
            service_tier,
        });
    }
    Ok(routes)
}

pub(super) fn retry_policy(routing: &Value) -> Result<RetryPolicy, String> {
    let defaults = RetryPolicy::default();
    let retry = routing.get("retry").unwrap_or(&Value::Null);
    if !retry.is_null() && !retry.is_object() {
        return Err("modelRouting.retry must be an object".into());
    }
    let max_attempts = match retry.get("maxAttempts") {
        None => defaults.max_attempts,
        Some(raw) => raw
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=8).contains(value))
            .ok_or("modelRouting.retry.maxAttempts must be an integer from 1 through 8")?,
    };
    let jitter_ratio = match retry.get("jitterRatio") {
        None => defaults.jitter_ratio,
        Some(raw) => raw
            .as_f64()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or("modelRouting.retry.jitterRatio must be between 0 and 1")?,
    };
    Ok(RetryPolicy {
        max_attempts,
        base_delay: duration_setting(retry, "baseDelayMs", defaults.base_delay.as_millis() as u64)?,
        max_delay: duration_setting(retry, "maxDelayMs", defaults.max_delay.as_millis() as u64)?,

        jitter_ratio,
    })
}
