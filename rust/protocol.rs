use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action")]
pub enum Action {
    #[serde(rename = "final")]
    Final { text: String },
    #[serde(rename = "tool")]
    Tool { tool: String, input: Value },
    #[serde(rename = "tools")]
    Tools { tools: Vec<ToolAction> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolAction {
    pub tool: String,
    pub input: Value,
}

pub fn extract_json_object(text: &str) -> Result<&str, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err("model returned empty content".into());
    }
    if raw.starts_with('{') && raw.ends_with('}') {
        return Ok(raw);
    }
    let start = raw.find('{').ok_or_else(|| format!("model returned non-json content: {}", raw.chars().take(200).collect::<String>()))?;
    let end = raw.rfind('}').ok_or_else(|| format!("model returned non-json content: {}", raw.chars().take(200).collect::<String>()))?;
    if end <= start {
        return Err(format!("model returned non-json content: {}", raw.chars().take(200).collect::<String>()));
    }
    Ok(&raw[start..=end])
}

fn parse_tool_action(value: &Value) -> Result<ToolAction, String> {
    let tool = value.get("tool").and_then(Value::as_str).ok_or("tool action requires tool")?;
    let input = value.get("input").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
    Ok(ToolAction { tool: tool.to_string(), input })
}

pub fn parse_action(text: &str) -> Result<Action, String> {
    let json_text = extract_json_object(text)?;
    let value: Value = serde_json::from_str(json_text).map_err(|e| e.to_string())?;
    if !value.is_object() {
        return Err("action must be a JSON object".into());
    }
    match value.get("action").and_then(Value::as_str).unwrap_or("") {
        "final" => {
            let text = value.get("text").and_then(Value::as_str).ok_or("final action requires text")?;
            Ok(Action::Final { text: text.to_string() })
        }
        "tool" => {
            let action = parse_tool_action(&value)?;
            Ok(Action::Tool { tool: action.tool, input: action.input })
        }
        "tools" => {
            let raw_tools = value.get("tools").and_then(Value::as_array).ok_or("tools action requires tools")?;
            if raw_tools.is_empty() {
                return Err("tools action requires tools".into());
            }
            let mut tools = Vec::with_capacity(raw_tools.len());
            for item in raw_tools {
                tools.push(parse_tool_action(item)?);
            }
            Ok(Action::Tools { tools })
        }
        other => Err(format!("unknown action: {other}")),
    }
}

#[allow(dead_code)]
pub fn format_tool_result(result: &Value) -> String {
    json!({"type": "tool_result", "result": result}).to_string()
}
