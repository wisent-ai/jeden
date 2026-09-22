//! What one model call cost, and where that is written down.
//!
//! Split out of `agent/runtime/routing.rs`, which had grown past the module
//! line cap.

use super::super::*;
use crate::routing::SubscriptionTarget;

fn usage_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/usage.json")
}

pub(in crate::agent) fn usage_cost(
    cwd: &Path,
    _config: &Config,
    model: &str,
    usage: &CompletionUsage,
) -> Option<Value> {
    let endpoint = env::var("BRAMA_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let client = crate::control_plane::brama::BramaClient::configured(
        endpoint,
        env::var("BRAMA_TOKEN").ok(),
    );
    let catalog = crate::control_plane::model_catalog(cwd, &client, false).ok()?;
    let cost = catalog.price(model)?;
    let input = usage.input_tokens * cost.input / 1_000_000.0;
    let output = usage.output_tokens * cost.output / 1_000_000.0;
    let cache_read = usage.cache_read_tokens * cost.cache_read / 1_000_000.0;
    let cache_write = usage.cache_write_tokens * cost.cache_write / 1_000_000.0;
    Some(json!({
        "input": input,
        "output": output,
        "cacheRead": cache_read,
        "cacheWrite": cache_write,
        "total": input + output + cache_read + cache_write,
    }))
}

pub(in crate::agent) fn append_usage_event(
    cwd: &Path,
    router: &ChatConfig,
    usage: &CompletionUsage,
    cost: Option<Value>,
    subscription_target: Option<&SubscriptionTarget>,
    subscription_decision_id: Option<&str>,
) -> Result<(), String> {
    let path = usage_path(cwd);
    let mut document = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({"version": 1, "events": []}));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let events = document
        .as_object_mut()
        .ok_or("usage document must be a JSON object")?
        .entry("events")
        .or_insert_with(|| json!([]));
    let events = events
        .as_array_mut()
        .ok_or("usage events must be an array")?;
    let mut event = json!({
        "at": now_stamp(),
        "model": router.model.clone(),
        "serviceTier": if router.service_tier.trim().is_empty() { Value::Null } else { json!(router.service_tier.clone()) },
        "inputTokens": usage.input_tokens,
        "outputTokens": usage.output_tokens,
        "cacheReadTokens": usage.cache_read_tokens,
        "cacheWriteTokens": usage.cache_write_tokens,
        "totalTokens": usage.total_tokens,
    });
    if let Some(cost) = cost {
        event["cost"] = cost;
    }
    if let Some(target) = subscription_target {
        event["billing"] = json!({
            "providerId": target.provider_id,
            "accountId": target.account_id,
            "subscriptionId": target.subscription_id,
            "quotaBucket": target.quota_bucket,
            "decisionId": subscription_decision_id,
        });
    }
    events.push(event);
    if let Some(obj) = document.as_object_mut() {
        obj.insert("version".into(), json!(1));
        obj.insert("updatedAt".into(), json!(now_stamp()));
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(&document).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())
}
