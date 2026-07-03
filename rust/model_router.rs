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
    let data: Value = serde_json::from_str(&text).map_err(|e| format!("invalid model router JSON: {e}"))?;
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
