use super::*;
use crate::control_plane::billing::{AccountState, SubscriptionState, MAX_BILLING_ITEMS};
use crate::control_plane::contract::{RequestMeta, WelesApiV2};
use crate::control_plane::weles::{platform_billing_configured, WelesClient};
use crate::model_router::{RetryPolicy, RouteDescriptor};
use crate::routing::{SubscriptionPoolSnapshot, SubscriptionTarget};
use sha2::{Digest, Sha256};

const MAX_CONFIGURED_ROUTES: usize = 16;

fn duration_setting(
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

fn route_descriptors(value: Option<&Value>, key: &str) -> Result<Vec<RouteDescriptor>, String> {
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

fn retry_policy(routing: &Value) -> Result<RetryPolicy, String> {
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
        first_event_timeout: duration_setting(
            retry,
            "firstEventTimeoutMs",
            defaults.first_event_timeout.as_millis() as u64,
        )?,
        idle_timeout: duration_setting(
            retry,
            "idleTimeoutMs",
            defaults.idle_timeout.as_millis() as u64,
        )?,
        jitter_ratio,
    })
}

fn subscription_pool_from_platform_billing() -> Result<Option<SubscriptionPoolSnapshot>, String> {
    if !platform_billing_configured() {
        return Ok(None);
    }
    let client = WelesClient::from_env();
    let accounts = client
        .accounts(None)
        .map_err(|error| format!("cannot list platform billing accounts for subscription routing: {error}"))?;
    let mut targets = Vec::new();
    let mut revisions = Vec::new();
    'accounts: for (account_index, account) in accounts.into_iter().enumerate() {
        if account.status != "active" {
            continue;
        }
        let correlation = format!("subscription-discovery-{account_index}");
        let Ok(status) = client.billing_status(
            &account.id,
            &RequestMeta::read_v2(format!("{correlation}-status")),
        ) else {
            continue;
        };
        if status.status != AccountState::Active || status.provider_id != account.provider {
            continue;
        }
        let Ok(subscriptions) = client.subscriptions(
            &account.id,
            &RequestMeta::read_v2(format!("{correlation}-subscriptions")),
        ) else {
            continue;
        };
        for (subscription_index, subscription) in subscriptions.into_iter().enumerate() {
            if subscription.status != SubscriptionState::Active
                || subscription.provider_id != status.provider_id
            {
                continue;
            }
            let Ok(quota) = client.quota(
                &subscription.id,
                &RequestMeta::read_v2(format!("{correlation}-quota-{subscription_index}")),
            ) else {
                continue;
            };
            let limiting_bucket = quota.buckets.into_iter().min_by(|left, right| {
                let rank = |bucket: &crate::control_plane::billing::QuotaBucket| {
                    use crate::control_plane::billing::QuotaState;
                    match bucket.state {
                        QuotaState::Exhausted => 0_u8,
                        QuotaState::Unknown => 1,
                        QuotaState::Available => 2,
                        QuotaState::Unmetered => 3,
                    }
                };
                rank(left).cmp(&rank(right)).then_with(|| {
                    match (
                        left.remaining.zip(left.limit),
                        right.remaining.zip(right.limit),
                    ) {
                        (
                            Some((left_remaining, left_limit)),
                            Some((right_remaining, right_limit)),
                        ) if left_limit > 0 && right_limit > 0 => (u128::from(left_remaining)
                            * u128::from(right_limit))
                        .cmp(&(u128::from(right_remaining) * u128::from(left_limit))),
                        _ => left.bucket_id.cmp(&right.bucket_id),
                    }
                })
            });
            let Some(bucket) = limiting_bucket else {
                continue;
            };
            revisions.push(quota.revision);
            targets.push(SubscriptionTarget {
                provider_id: subscription.provider_id,
                account_id: account.id.clone(),
                subscription_id: subscription.id,
                quota_bucket: bucket.bucket_id,
                priority: 0,
                quota_state: bucket.state,
                remaining: bucket.remaining,
                limit: bucket.limit,
                capabilities: ["chat".to_string()].into_iter().collect(),
                active: true,
                valid_until_ms: u64::MAX,
                policy_allowed: true,
            });
            if targets.len() >= MAX_BILLING_ITEMS {
                break 'accounts;
            }
        }
    }
    if targets.is_empty() {
        return Ok(None);
    }
    targets.sort_by_key(SubscriptionTarget::identity);
    revisions.sort();
    revisions.dedup();
    let encoded = serde_json::to_vec(&(revisions, &targets)).map_err(|error| error.to_string())?;
    let revision = hex::encode(Sha256::digest(encoded));
    Ok(Some(SubscriptionPoolSnapshot {
        revision,
        rendezvous_salt: "weles-subscription-routing-v1".into(),
        targets,
    }))
}

/// Fetch the Brama catalog with bounded retries (max 2 extra attempts, ~2s
/// then ~8s) on transient failures only — transport errors and HTTP 429/5xx —
/// so a momentary outage does not hard-fail the run before the first chat
/// call. Validation and schema errors surface immediately.
fn model_catalog_with_retry(
    cwd: &Path,
    client: &crate::control_plane::brama::BramaClient,
) -> Result<crate::control_plane::brama::ModelCatalog, crate::control_plane::brama::BramaError> {
    use crate::control_plane::brama::BramaError;
    const DELAYS: [std::time::Duration; 2] = [
        std::time::Duration::from_secs(2),
        std::time::Duration::from_secs(8),
    ];
    for attempt in 0..=DELAYS.len() {
        match crate::control_plane::model_catalog(cwd, client, false) {
            Ok(catalog) => return Ok(catalog),
            Err(error) => {
                let transient = match &error {
                    BramaError::Transport(_) | BramaError::RateLimited { .. } => true,
                    BramaError::Http { status, .. } => *status == 429 || (500..600).contains(status),
                    _ => false,
                };
                if !transient || attempt == DELAYS.len() {
                    return Err(error);
                }
                eprintln!("retry {}/{} after {}", attempt + 1, DELAYS.len(), error);
                std::thread::sleep(DELAYS[attempt]);
            }
        }
    }
    unreachable!("catalog retry loop returns on the final attempt")
}

pub(crate) fn model_router_config(config: &Config, args: &Args) -> ChatConfig {
    let mode_state = read_mode_state(&args.cwd);
    let mode_service_tier = if mode_state
        .pointer("/fast/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        mode_state
            .pointer("/fast/serviceTier")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    } else {
        None
    };
    let merged = crate::cli::config::merged_config_value(&args.cwd);
    let routing = merged.get("modelRouting").unwrap_or(&Value::Null);
    let retry = retry_policy(routing);
    let configured_fallbacks = route_descriptors(routing.get("fallbacks"), "fallbacks");
    let configured_promotions =
        route_descriptors(routing.get("contextPromotions"), "contextPromotions");
    let endpoint = env::var("BRAMA_URL")
        .ok()
        .or(config.model_router_url.clone())
        .filter(|value| !value.trim().is_empty());
    let selected_model = args
        .model
        .clone()
        .or(config.model.clone())
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .filter(|value| !value.trim().is_empty());
    let catalog_client = crate::control_plane::brama::BramaClient::configured(
        endpoint.clone(),
        env::var("BRAMA_TOKEN").ok(),
    );
    let catalog = model_catalog_with_retry(&args.cwd, &catalog_client);
    // Bare (provider-less) model ids resolve to the unique catalog route whose
    // id ends with `/<model>`; an ambiguous id names every matching route.
    let mut bare_model_error = None;
    let selected_model = match (selected_model, &catalog) {
        (Some(model), Ok(catalog))
            if !model.contains('/')
                && !crate::model_router::is_virtual_model_route(&model)
                && !catalog.models.iter().any(|entry| entry.id == model) =>
        {
            match catalog.resolve_bare(&model) {
                Ok(Some(entry)) => Some(entry.id.clone()),
                Ok(None) => Some(model),
                Err(error) => {
                    bare_model_error = Some(error);
                    Some(model)
                }
            }
        }
        (model, _) => model,
    };
    let subscription_pool = subscription_pool_from_platform_billing();
    let catalog_error = bare_model_error.or(match (&selected_model, &catalog) {
        (None, _) => Some(
            "no model selected; choose a model advertised by Brama; run /setup to configure"
                .to_string(),
        ),
        (_, Err(error)) => Some(error.to_string()),
        (Some(model), Ok(_)) if crate::model_router::is_virtual_model_route(model) => None,
        (Some(model), Ok(catalog)) => catalog.resolve(model).err().map(|error| error.to_string()),
    });
    let image_capable_models = catalog
        .as_ref()
        .map(|catalog| {
            catalog
                .models
                .iter()
                .filter(|entry| {
                    entry.available
                        && entry
                            .input_modalities
                            .iter()
                            .any(|modality| modality.eq_ignore_ascii_case("image"))
                })
                .map(|entry| entry.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let validate_routes = |routes: &Result<Vec<RouteDescriptor>, String>| -> Option<String> {
        let catalog = catalog.as_ref().ok()?;
        routes.as_ref().ok()?.iter().find_map(|route| {
            if crate::model_router::is_virtual_model_route(&route.model) {
                None
            } else {
                catalog
                    .resolve(&route.model)
                    .err()
                    .map(|error| error.to_string())
            }
        })
    };
    let config_error = retry
        .as_ref()
        .err()
        .cloned()
        .or_else(|| configured_fallbacks.as_ref().err().cloned())
        .or_else(|| configured_promotions.as_ref().err().cloned())
        .or(catalog_error)
        .or_else(|| validate_routes(&configured_fallbacks))
        .or_else(|| validate_routes(&configured_promotions));
    let catalog_routes = |fallback: bool| -> Vec<RouteDescriptor> {
        let Some(model) = selected_model.as_deref() else {
            return Vec::new();
        };
        let Ok(catalog) = &catalog else {
            return Vec::new();
        };
        let Ok(entry) = catalog.resolve(model) else {
            return Vec::new();
        };
        let ids = if fallback {
            &entry.fallback
        } else {
            &entry.promotion
        };
        ids.iter()
            .filter(|id| catalog.resolve(id).is_ok())
            .map(|id| RouteDescriptor {
                model: id.clone(),
                service_tier: None,
            })
            .collect()
    };
    let fallbacks = configured_fallbacks.unwrap_or_default();
    let promotions = configured_promotions.unwrap_or_default();
    let resolved_fallbacks = if fallbacks.is_empty() {
        catalog_routes(true)
    } else {
        fallbacks
    };
    let resolved_promotions = if promotions.is_empty() {
        catalog_routes(false)
    } else {
        promotions
    };
    let subscription_pool = subscription_pool.unwrap_or(None);
    let subscription_cooldown_path = subscription_pool
        .as_ref()
        .map(|_| args.cwd.join(".jeden/subscription-cooldowns.json"));
    ChatConfig {
        url: endpoint.unwrap_or_default(),
        agent_id: env::var("WISENT_APP_AGENT_ID")
            .ok()
            .or(config.agent_id.clone())
            .unwrap_or_else(|| "wisent-app".into()),
        secret: env::var("WISENT_APP_AGENT_AUTH_SECRET").unwrap_or_default(),
        model: selected_model.unwrap_or_default(),
        service_tier: env::var("JEDEN_SERVICE_TIER")
            .ok()
            .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
            .or(mode_service_tier)
            .unwrap_or_default(),
        retry: retry.unwrap_or_default(),
        fallbacks: resolved_fallbacks,
        context_promotions: resolved_promotions,
        image_capable_models,
        subscription_pool,
        subscription_cooldown_path,
        config_error,
    }
}

pub(in crate::agent) fn env_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub(in crate::agent) fn memory_guidance_for_prompt(cwd: &Path) -> Option<String> {
    let store =
        crate::memory::MemoryStore::open(crate::memory::MemoryStore::default_path()).ok()?;
    let scope = crate::memory::MemoryScope {
        kind: "repo".into(),
        id: cwd.display().to_string(),
    };
    let context = store.pre_compaction_context(&scope, "", 12_000).ok()?;
    (!context.is_empty()).then_some(context)
}

pub(in crate::agent) fn is_context_overflow_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("too many tokens")
        || lower.contains("tokens exceed")
}

pub(in crate::agent) fn is_incomplete_output_error(error: &str) -> bool {
    error.to_ascii_lowercase().contains("response incomplete")
}

fn usage_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/usage.json")
}

pub(in crate::agent) fn usage_cost(
    cwd: &Path,
    config: &Config,
    model: &str,
    usage: &CompletionUsage,
) -> Option<Value> {
    let endpoint = env::var("BRAMA_URL")
        .ok()
        .or(config.model_router_url.clone());
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
