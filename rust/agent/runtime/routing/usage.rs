//! What one model call cost, and where that is written down.
//!
//! Split out of `agent/runtime/routing.rs`, which had grown past the module
//! line cap.

use super::super::*;

fn usage_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/usage.json")
}

pub(in crate::agent) fn usage_cost(
    cwd: &Path,
    _config: &Config,
    model: &str,
    usage: &CompletionUsage,
) -> Option<Value> {
    let client = match crate::control_plane::model_client() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("jeden: the model route could not be read to price {model}; the call is recorded without a cost: {error}");
            return None;
        }
    };
    // An unpriced model and an unreadable catalogue both leave the call
    // without a cost; only the second is a failure, so it is said with its cause.
    let catalog = match crate::control_plane::model_catalog(cwd, &client, false) {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("jeden: the model catalog could not be read to price {model}; the call is recorded without a cost: {error}");
            return None;
        }
    };
    let cost = catalog.price(model)?;
    let counts = [
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
    ];
    let nothing: f64 = Default::default();
    let per_price = f64::from(crate::control_plane::brama::ModelPrice::TOKENS_PER_PRICE);
    let mut priced = serde_json::Map::new();
    let mut total = nothing;
    for ((side, rate), tokens) in cost.sides().into_iter().zip(counts) {
        let spent = match rate {
            Some(rate) => tokens * rate / per_price,
            // A side the call used no tokens of costs nothing at any rate.
            None if tokens <= nothing => nothing,
            None => {
                eprintln!("jeden: the catalogue states no {side} price for {model}, so the call's {tokens} {side} tokens cannot be priced; it is recorded without a cost");
                return None;
            }
        };
        priced.insert(side.to_string(), json!(spent));
        total += spent;
    }
    priced.insert("total".into(), json!(total));
    Some(Value::Object(priced))
}

pub(in crate::agent) fn append_usage_event(
    cwd: &Path,
    router: &ChatConfig,
    usage: &CompletionUsage,
    cost: Option<Value>,
    lease: Option<&crate::model_router::lease::SessionLease>,
) -> Result<(), String> {
    let path = usage_path(cwd);
    // The time is read before the ledger: a clock that cannot give one
    // leaves the ledger as it was, never stamped as the epoch.
    let at = now_stamp()?;
    // Only a ledger that is not there starts empty: one that cannot be read
    // or is not JSON was replaced by this single event, losing every event
    // recorded before it.
    let mut document = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|error| {
            format!(
                "usage ledger {} is not JSON, so it is not rewritten: {error}",
                path.display()
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json!({"events": []})
        }
        Err(error) => {
            return Err(format!(
                "cannot read usage ledger {}: {error}",
                path.display()
            ))
        }
    };
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
        "at": at.clone(),
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
    if let Some(lease) = lease {
        event["billing"] = json!({
            "providerId": lease.provider,
            "accountId": lease.account,
            "subscriptionId": lease.subscription_id,
            "leaseId": lease.id,
        });
    }
    events.push(event);
    if let Some(obj) = document.as_object_mut() {
        obj.insert("updatedAt".into(), json!(at));
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(&document).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())
}
