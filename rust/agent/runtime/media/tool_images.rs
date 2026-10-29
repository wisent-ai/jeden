use serde_json::{json, Value};

/// Only the provider copy gains image parts. Recorded tool receipts and the
/// conversation remain unchanged and keep their original image bytes.
pub(super) fn attach(messages: &mut [Value]) -> Result<(), String> {
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        // The text that may carry a tool result: the content itself, or its
        // first text part, kept by position so that part is the one rewritten.
        let (text, slot) = match &*content {
            Value::String(text) => (text.as_str(), None),
            Value::Array(parts) => {
                let Some((index, text)) = parts.iter().enumerate().find_map(|(index, part)| {
                    (part.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| part.get("text").and_then(Value::as_str))
                        .flatten()
                        .map(|text| (index, text))
                }) else {
                    continue;
                };
                (text, Some(index))
            }
            _ => continue,
        };
        // A user message that is not a JSON object is prose, not a tool result.
        if !text.starts_with('{') {
            continue;
        }
        let Ok(mut envelope) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if envelope.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let Some(result) = envelope.get_mut("result") else {
            continue;
        };
        let mut images = Vec::new();
        collect(result, &mut images)?;
        if images.is_empty() {
            continue;
        }
        let rewritten = envelope.to_string();
        match (content, slot) {
            (Value::Array(parts), Some(index)) => {
                parts[index]["text"] = Value::String(rewritten);
                parts.append(&mut images);
            }
            (content, _) => {
                let mut parts = vec![json!({"type": "text", "text": rewritten})];
                parts.append(&mut images);
                *content = Value::Array(parts);
            }
        }
    }
    Ok(())
}

fn collect(result: &mut Value, images: &mut Vec<Value>) -> Result<(), String> {
    match result {
        Value::Array(values) => {
            for value in values {
                collect(value, images)?;
            }
        }
        Value::Object(fields) => {
            if fields.get("ok").and_then(Value::as_bool) == Some(false) {
                return Ok(());
            }
            let mime = fields
                .get("mimeType")
                .and_then(Value::as_str)
                .filter(|mime| {
                    matches!(
                        *mime,
                        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                    )
                });
            let complete = fields.contains_key("base64")
                && fields.contains_key("width")
                && fields.contains_key("height");
            if let Some(mime) = mime.filter(|_mime| complete) {
                // The result is named by its path in a refusal; one without a
                // path is said to have none.
                let named = match fields.get("path").and_then(Value::as_str) {
                    Some(path) => format!("`{path}`"),
                    None => "with no path".to_string(),
                };
                if fields.get("truncated").and_then(Value::as_bool) == Some(true) {
                    return Err(format!("image tool result {named} is truncated; incomplete image bytes cannot be sent to the model"));
                }
                // The prefix is written while the MIME type still borrows the
                // fields, before the base64 content is taken out of them.
                let prefix = format!("data:{mime};base64,");
                let Some(Value::String(encoded)) = fields.remove("base64") else {
                    return Err(format!(
                        "image tool result {named} base64 content is not a string"
                    ));
                };
                if encoded.is_empty() {
                    return Err(format!(
                        "image tool result {named} has empty base64 content"
                    ));
                }
                let url = format!("{prefix}{encoded}");
                images.push(json!({"type": "image_url", "image_url": {"url": url}}));
            } else {
                for value in fields.values_mut() {
                    collect(value, images)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
