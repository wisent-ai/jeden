use super::*;

pub(crate) fn model_router_config(config: &Config, args: &Args) -> ChatConfig {
    let mode_state = read_mode_state(&args.cwd);
    let mode_service_tier = if mode_state.pointer("/fast/enabled").and_then(Value::as_bool).unwrap_or(false) {
        mode_state.pointer("/fast/serviceTier").and_then(Value::as_str).filter(|value| !value.trim().is_empty()).map(str::to_string)
    } else {
        None
    };
    ChatConfig {
        url: env::var("MODEL_ROUTER_URL")
            .ok()
            .or(config.model_router_url.clone())
            .unwrap_or_else(|| "https://model-router-1080673333190.us-central1.run.app".into()),
        agent_id: env::var("WISENT_APP_AGENT_ID")
            .ok()
            .or(config.agent_id.clone())
            .unwrap_or_else(|| "wisent-app".into()),
        secret: env::var("WISENT_APP_AGENT_AUTH_SECRET").unwrap_or_default(),
        model: args
            .model
            .clone()
            .or(config.model.clone())
            .or_else(|| env::var("JEDEN_MODEL").ok())
            .unwrap_or_else(|| "claude-code-subscription".into()),
        service_tier: env::var("JEDEN_SERVICE_TIER")
            .ok()
            .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
            .or(mode_service_tier)
            .unwrap_or_default(),
    }
}

pub(in crate::agent) fn env_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn memory_summary_path(cwd: &Path) -> PathBuf {
    env::var_os("JEDEN_MEMORY_SUMMARY_FILE").map(PathBuf::from).unwrap_or_else(|| cwd.join(".jeden/memory_summary.md"))
}

pub(in crate::agent) fn memory_guidance_for_prompt(cwd: &Path) -> Option<String> {
    let raw = fs::read_to_string(memory_summary_path(cwd)).ok()?;
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        None
    } else if compact.chars().count() > 20_000 {
        Some(compact.chars().take(20_000).collect::<String>())
    } else {
        Some(compact)
    }
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

pub(in crate::agent) fn usage_cost(config: &Config, model: &str, usage: &CompletionUsage) -> Option<Value> {
    let base = config.models.iter().find(|entry| entry.id == model).and_then(|entry| entry.cost.as_ref());
    let override_cost = config.model_overrides.get(model).and_then(|entry| entry.cost.as_ref());
    let cost = override_cost.or(base)?;
    let input = usage.input_tokens * cost.input.unwrap_or(0.0) / 1_000_000.0;
    let output = usage.output_tokens * cost.output.unwrap_or(0.0) / 1_000_000.0;
    let cache_read = usage.cache_read_tokens * cost.cache_read.unwrap_or(0.0) / 1_000_000.0;
    let cache_write = usage.cache_write_tokens * cost.cache_write.unwrap_or(0.0) / 1_000_000.0;
    Some(json!({
        "input": input,
        "output": output,
        "cacheRead": cache_read,
        "cacheWrite": cache_write,
        "total": input + output + cache_read + cache_write,
    }))
}

pub(in crate::agent) fn append_usage_event(cwd: &Path, router: &ChatConfig, usage: &CompletionUsage, cost: Option<Value>) -> Result<(), String> {
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
    let events = events.as_array_mut().ok_or("usage events must be an array")?;
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
    events.push(event);
    if let Some(obj) = document.as_object_mut() {
        obj.insert("version".into(), json!(1));
        obj.insert("updatedAt".into(), json!(now_stamp()));
    }
    fs::write(&path, serde_json::to_string_pretty(&document).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
}
