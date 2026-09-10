use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action")]
pub enum Action {
    #[serde(rename = "final")]
    Final {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        report: Option<Value>,
    },
    #[serde(rename = "message")]
    Message { text: String },
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

/// Prefix of every refusal for an answer that stopped before it was whole.
///
/// A cut-off answer used to reach the caller as serde's own `EOF while parsing
/// a string at line 1 column 1440`, a column in text nobody can see. On
/// 2026-09-10 that sentence ended a whole assignment, because the answer it
/// described was one truncated intake plan. The prefix lets a turn recognise
/// the shape of the failure and say what happened to the answer.
pub const INCOMPLETE_ANSWER: &str = "model answer stopped mid-JSON";

/// Prefix of every refusal for content that carries no JSON object at all.
/// Prose is a legitimate answer, so this refusal is not a failure everywhere.
pub const NON_JSON_ANSWER: &str = "model returned non-json content";

pub fn is_incomplete_answer(message: &str) -> bool {
    message.starts_with(INCOMPLETE_ANSWER)
}

fn non_json(raw: &str) -> String {
    format!(
        "{NON_JSON_ANSWER}: {}",
        raw.chars().take(200).collect::<String>()
    )
}

/// The first complete JSON object in `text`, or why there is none.
///
/// The scan tracks strings and escapes, so a brace inside a string value never
/// closes an object, and an answer that ends mid-object is reported as
/// truncated instead of being sliced at its last brace - which is how a cut
/// answer used to arrive at serde as an unterminated string.
pub fn extract_json_object(text: &str) -> Result<&str, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err("model returned empty content".into());
    }
    let Some(start) = raw.find('{') else {
        return Err(non_json(raw));
    };
    let mut depth = usize::default();
    let mut in_string = false;
    let mut escaped = false;
    for (offset, character) in raw[start..].char_indices() {
        if in_string {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' | '[' => depth += usize::from(true),
            '}' | ']' => {
                depth = depth.saturating_sub(usize::from(true));
                if depth == usize::default() {
                    return Ok(&raw[start..start + offset + character.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    let bytes = raw.len() - start;
    Err(if in_string {
        format!("{INCOMPLETE_ANSWER} after {bytes} bytes: a JSON string is never closed")
    } else {
        format!("{INCOMPLETE_ANSWER} after {bytes} bytes: {depth} bracket(s) are never closed")
    })
}

fn parse_tool_action(value: &Value) -> Result<ToolAction, String> {
    let tool = value
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("tool action requires tool")?;
    let input = value
        .get("input")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(ToolAction {
        tool: tool.to_string(),
        input,
    })
}

pub fn parse_action(text: &str) -> Result<Action, String> {
    let json_text = extract_json_object(text)?;
    let value: Value = serde_json::from_str(json_text)
        .map_err(|error| format!("model answer is not valid JSON: {error}"))?;
    if !value.is_object() {
        return Err("action must be a JSON object".into());
    }
    match value.get("action").and_then(Value::as_str).unwrap_or("") {
        "final" => {
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .ok_or("final action requires text")?;
            Ok(Action::Final {
                text: text.to_string(),
                report: value.get("report").cloned(),
            })
        }
        "message" => {
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or("message action requires nonempty text")?;
            Ok(Action::Message {
                text: text.to_string(),
            })
        }
        "tool" => {
            let action = parse_tool_action(&value)?;
            Ok(Action::Tool {
                tool: action.tool,
                input: action.input,
            })
        }
        "tools" => {
            let raw_tools = value
                .get("tools")
                .and_then(Value::as_array)
                .ok_or("tools action requires tools")?;
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
