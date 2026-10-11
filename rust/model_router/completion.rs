use super::*;

pub fn hmac_headers(
    body: &str,
    agent_id: &str,
    secret: &str,
) -> Result<(String, String, String), String> {
    if secret.is_empty() {
        return Err("WISENT_APP_AGENT_AUTH_SECRET is required".into());
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let body_hash = if body.is_empty() {
        String::new()
    } else {
        hex::encode(Sha256::digest(body.as_bytes()))
    };
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(format!("{}:{}:{}", agent_id, ts, body_hash).as_bytes());
    Ok((ts, body_hash, hex::encode(mac.finalize().into_bytes())))
}

/// The headers that prove to Brama which agent is calling. A provider named
/// by `JEDEN_MODEL_ENDPOINT` gets none: its configuration carries no signing
/// secret, and routing refuses a Brama run that has none.
pub(crate) fn agent_headers(
    body: &str,
    agent_id: &str,
    secret: &str,
) -> Result<Vec<(&'static str, String)>, String> {
    if secret.is_empty() {
        return Ok(Vec::new());
    }
    let (ts, body_hash, signature) = hmac_headers(body, agent_id, secret)?;
    Ok(vec![
        ("x-agent-id", agent_id.to_string()),
        ("x-agent-timestamp", ts),
        ("x-agent-body-sha256", body_hash),
        ("x-agent-signature", signature),
    ])
}

pub(crate) fn tool_calls_to_action(tool_calls: &[Value]) -> Result<String, String> {
    let mut actions = Vec::new();
    for call in tool_calls {
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.trim().is_empty() {
            return Err("model router returned tool call without function name".into());
        }
        let raw_args = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        let input: Value = if raw_args.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw_args)
                .map_err(|e| format!("invalid tool arguments for {name}: {e}"))?
        };
        actions.push(json!({"tool": name, "input": input}));
    }
    if actions.len() == 1 {
        Ok(
            json!({"action": "tool", "tool": actions[0]["tool"], "input": actions[0]["input"]})
                .to_string(),
        )
    } else {
        Ok(json!({"action": "tools", "tools": actions}).to_string())
    }
}

/// Parse a full (non-streamed) completion body into an action string / content.
pub(crate) fn value_number(value: &Value, paths: &[&str]) -> f64 {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_f64))
        .unwrap_or(0.0)
}

pub(crate) fn usage_from_value(data: &Value) -> Option<CompletionUsage> {
    let usage = data.get("usage")?;
    let input_tokens = value_number(usage, &["/input_tokens", "/prompt_tokens", "/input"]);
    let output_tokens = value_number(usage, &["/output_tokens", "/completion_tokens", "/output"]);
    let cache_read_tokens = value_number(
        usage,
        &[
            "/cache_read_tokens",
            "/cacheRead",
            "/prompt_tokens_details/cached_tokens",
        ],
    );
    let cache_write_tokens = value_number(usage, &["/cache_write_tokens", "/cacheWrite"]);
    let total_tokens = value_number(usage, &["/total_tokens", "/totalTokens", "/total"]);
    let total_tokens = if total_tokens > 0.0 {
        total_tokens
    } else {
        input_tokens + output_tokens + cache_read_tokens + cache_write_tokens
    };
    if input_tokens == 0.0
        && output_tokens == 0.0
        && cache_read_tokens == 0.0
        && cache_write_tokens == 0.0
        && total_tokens == 0.0
    {
        None
    } else {
        Some(CompletionUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
        })
    }
}

/// Terminal messages for a successful-but-empty router response; both are
/// transient — a retry can legitimately produce content.
pub(crate) const NO_MESSAGE: &str = "model router returned no message";
pub(crate) const NO_MESSAGE_CONTENT: &str = "model router returned no message content";

/// Parse a full (non-streamed) completion body into an action string / content.
pub(crate) fn parse_completion_response(text: &str) -> Result<Completion, String> {
    let data: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid model router JSON: {e}"))?;
    let usage = usage_from_value(&data);
    let message = data.pointer("/choices/0/message").ok_or(NO_MESSAGE)?;
    let finish_reason = data
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    if matches!(finish_reason, "length" | "max_tokens") {
        return Err("model response incomplete: length".into());
    }
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !tool_calls.is_empty() {
        return Ok(Completion {
            content: tool_calls_to_action(&tool_calls)?,
            usage,
        });
    }
    let content = message.get("content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Err(NO_MESSAGE_CONTENT.into());
    }
    Ok(Completion {
        content: content.to_string(),
        usage,
    })
}
