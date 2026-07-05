use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ChatConfig {
    pub url: String,
    pub agent_id: String,
    pub secret: String,
    pub model: String,
    pub service_tier: String,
}

pub fn hmac_headers(body: &str, agent_id: &str, secret: &str) -> Result<(String, String, String), String> {
    if secret.is_empty() {
        return Err("WISENT_APP_AGENT_AUTH_SECRET is required".into());
    }
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs().to_string();
    let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(format!("{}:{}:{}", agent_id, ts, body_hash).as_bytes());
    Ok((ts, body_hash, hex::encode(mac.finalize().into_bytes())))
}

fn tool_calls_to_action(tool_calls: &[Value]) -> Result<String, String> {
    let mut actions = Vec::new();
    for call in tool_calls {
        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
        if name.trim().is_empty() {
            return Err("model router returned tool call without function name".into());
        }
        let raw_args = call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
        let input: Value = if raw_args.trim().is_empty() { json!({}) } else { serde_json::from_str(raw_args).map_err(|e| format!("invalid tool arguments for {name}: {e}"))? };
        actions.push(json!({"tool": name, "input": input}));
    }
    if actions.len() == 1 {
        Ok(json!({"action": "tool", "tool": actions[0]["tool"], "input": actions[0]["input"]}).to_string())
    } else {
        Ok(json!({"action": "tools", "tools": actions}).to_string())
    }
}

pub fn chat_completion(config: &ChatConfig, messages: Vec<Value>, max_tokens: usize, tools: &[Value]) -> Result<String, String> {
    let mut body = json!({
        "model": config.model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    if !config.service_tier.trim().is_empty() {
        body["service_tier"] = Value::String(config.service_tier.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = Value::String("auto".into());
    }
    let body_text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let (ts, body_hash, sig) = hmac_headers(&body_text, &config.agent_id, &config.secret)?;
    let client = Client::builder().timeout(Duration::from_secs(120)).build().map_err(|e| e.to_string())?;
    let response = client
        .post(format!("{}/v1/chat/completions", config.url.trim_end_matches('/')))
        .header("content-type", "application/json")
        .header("x-agent-id", &config.agent_id)
        .header("x-agent-timestamp", ts)
        .header("x-agent-body-sha256", body_hash)
        .header("x-agent-signature", sig)
        .body(body_text)
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("model router {}: {}", status.as_u16(), text.chars().take(800).collect::<String>()));
    }
    parse_completion_response(&text)
}

/// Parse a full (non-streamed) completion body into an action string / content.
fn parse_completion_response(text: &str) -> Result<String, String> {
    let data: Value = serde_json::from_str(text).map_err(|e| format!("invalid model router JSON: {e}"))?;
    let message = data.pointer("/choices/0/message").ok_or("model router returned no message")?;
    let tool_calls = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
    if !tool_calls.is_empty() {
        return tool_calls_to_action(&tool_calls);
    }
    let content = message.get("content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Err("model router returned no message content".into());
    }
    Ok(content.to_string())
}

/// Streaming chat completion. Requests SSE (`stream: true`); for each content
/// delta it calls `on_delta`. Falls back to whole-body parsing if the endpoint
/// ignores `stream` and returns a normal JSON completion. Tool-call responses
/// are accumulated and returned as an action string (no partial tool deltas are
/// surfaced). Returns the same action/content string as `chat_completion`.
pub fn chat_completion_streaming(
    config: &ChatConfig,
    messages: Vec<Value>,
    max_tokens: usize,
    tools: &[Value],
    on_delta: &mut dyn FnMut(&str),
) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    let mut body = json!({
        "model": config.model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });
    if !config.service_tier.trim().is_empty() {
        body["service_tier"] = Value::String(config.service_tier.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = Value::String("auto".into());
    }
    let body_text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let (ts, body_hash, sig) = hmac_headers(&body_text, &config.agent_id, &config.secret)?;
    let client = Client::builder().timeout(Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
    let response = client
        .post(format!("{}/v1/chat/completions", config.url.trim_end_matches('/')))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("x-agent-id", &config.agent_id)
        .header("x-agent-timestamp", ts)
        .header("x-agent-body-sha256", body_hash)
        .header("x-agent-signature", sig)
        .body(body_text)
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        return Err(format!("model router {}: {}", status.as_u16(), text.chars().take(800).collect::<String>()));
    }
    // Fallback: endpoint ignored `stream` and returned a normal JSON body.
    if !content_type.contains("event-stream") {
        let text = response.text().map_err(|e| e.to_string())?;
        let out = parse_completion_response(&text)?;
        return Ok(out);
    }
    // Parse the SSE stream: `data: {json}` lines, `[DONE]` terminates.
    let mut content = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let reader = BufReader::new(response);
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let payload = match line.strip_prefix("data:") {
            Some(rest) => rest.trim(),
            None => continue,
        };
        if payload == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(payload) else { continue; };
        let Some(delta) = chunk.pointer("/choices/0/delta") else { continue; };
        if let Some(piece) = delta.get("content").and_then(Value::as_str) {
            if !piece.is_empty() {
                content.push_str(piece);
                on_delta(piece);
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            accumulate_tool_call_deltas(&mut tool_calls, calls);
        }
    }
    if !tool_calls.is_empty() {
        return tool_calls_to_action(&tool_calls);
    }
    if content.trim().is_empty() {
        return Err("model router returned no message content".into());
    }
    Ok(content)
}

/// Merge OpenAI streaming tool-call deltas (indexed) into a growing list.
fn accumulate_tool_call_deltas(acc: &mut Vec<Value>, deltas: &[Value]) {
    for delta in deltas {
        let index = delta.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        while acc.len() <= index {
            acc.push(json!({"function": {"name": "", "arguments": ""}}));
        }
        let slot = &mut acc[index];
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            if !name.is_empty() {
                let cur = slot.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
                slot["function"]["name"] = Value::String(cur + name);
            }
        }
        if let Some(args) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            let cur = slot.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("").to_string();
            slot["function"]["arguments"] = Value::String(cur + args);
        }
    }
}
