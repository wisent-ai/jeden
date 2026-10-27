use super::*;

pub fn hmac_headers(
    body: &str,
    agent_id: &str,
    secret: &str,
) -> Result<(String, String, String), String> {
    if secret.is_empty() {
        return Err("WISENT_APP_AGENT_AUTH_SECRET is required".into());
    }
    // A clock before the epoch is refused: read as 0, it signed a timestamp
    // Brama refuses as stale, and the refusal named the signature.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("the system clock is before the Unix epoch: {error}"))?
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
        let Some(name) = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
        else {
            return Err(format!(
                "model router returned tool call without function name: {call}"
            ));
        };
        // Arguments the router did not state are refused, not read as `{}`:
        // a call run with no input was not the call the model made.
        let Some(raw_args) = call.pointer("/function/arguments").and_then(Value::as_str) else {
            return Err(format!(
                "model router returned tool call {name} without function arguments text: {call}"
            ));
        };
        let input: Value = if raw_args.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw_args)
                .map_err(|e| format!("invalid tool arguments for {name}: {e}"))?
        };
        actions.push(json!({"tool": name, "input": input}));
    }
    match actions.as_slice() {
        [only] => Ok(json!({"action": "tool", "tool": only["tool"], "input": only["input"]}).to_string()),
        _ => Ok(json!({"action": "tools", "tools": &actions}).to_string()),
    }
}

/// The usage an OpenAI chat-completions answer states — the wire Brama and
/// every endpoint Jeden asks answer on: `prompt_tokens` (its cached part
/// included), `completion_tokens`, `total_tokens`, and that cached part as
/// `prompt_tokens_details.cached_tokens` when the provider read one. A body
/// without usage, or a stream chunk whose usage is null, states none; a usage
/// missing a count is refused, never read as zero. This wire names no cache
/// writes, so none are counted.
pub(crate) fn usage_from_value(data: &Value) -> Result<Option<CompletionUsage>, String> {
    let usage = match data.get("usage") {
        None | Some(Value::Null) => return Ok(None),
        Some(usage) => usage,
    };
    let count = |pointer: &str| {
        usage
            .pointer(pointer)
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("model router usage states no {pointer}: {usage}"))
    };
    let prompt_tokens = count("/prompt_tokens")?;
    let output_tokens = count("/completion_tokens")?;
    let total_tokens = count("/total_tokens")?;
    let none = crate::tally::ORIGIN as f64;
    let cache_read_tokens = match usage.pointer("/prompt_tokens_details/cached_tokens") {
        None | Some(Value::Null) => none,
        Some(cached) => cached.as_f64().ok_or_else(|| {
            format!("model router usage states cached_tokens that is not a count: {usage}")
        })?,
    };
    // The cached tokens are a part of the prompt: priced once, at the cache
    // rate, and not again inside the input.
    let input_tokens = prompt_tokens - cache_read_tokens;
    if input_tokens < none {
        return Err(format!(
            "model router usage states more cached tokens than prompt tokens: {usage}"
        ));
    }
    Ok(Some(CompletionUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens: none,
        total_tokens,
    }))
}

/// Terminal messages for a successful-but-empty router response; both are
/// transient — a retry can legitimately produce content.
pub(crate) const NO_MESSAGE: &str = "model router returned no message";
pub(crate) const NO_MESSAGE_CONTENT: &str = "model router returned no message content";

/// Parse a full (non-streamed) completion body into an action string / content.
pub(crate) fn parse_completion_response(text: &str) -> Result<Completion, String> {
    let data: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid model router JSON: {e}"))?;
    let usage = usage_from_value(&data)?;
    let choice = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or(NO_MESSAGE)?;
    let message = choice.get("message").ok_or(NO_MESSAGE)?;
    // `length` is the chat-completions reason for an answer cut at its token
    // limit.
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err("model response incomplete: length".into());
    }
    if let Some(tool_calls) = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .filter(|calls| !calls.is_empty())
    {
        return Ok(Completion {
            content: tool_calls_to_action(tool_calls)?,
            usage,
        });
    }
    let Some(content) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
    else {
        return Err(NO_MESSAGE_CONTENT.into());
    };
    Ok(Completion {
        content: content.to_string(),
        usage,
    })
}
