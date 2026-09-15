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
        let text = match content {
            Value::String(text) => Some(text.as_str()),
            Value::Array(parts) => parts.iter().find_map(|part| {
                (part.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| part.get("text").and_then(Value::as_str))
                    .flatten()
            }),
            _ => None,
        };
        let Some(text) = text.filter(|text| text.starts_with('{')) else {
            continue;
        };
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
        let text = envelope.to_string();
        match content {
            Value::String(_) => {
                let mut parts = Vec::with_capacity(images.len() + 1);
                parts.push(json!({"type": "text", "text": text}));
                parts.append(&mut images);
                *content = Value::Array(parts);
            }
            Value::Array(parts) => {
                let part = parts
                    .iter_mut()
                    .find(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .expect("text part was found above");
                part["text"] = Value::String(text);
                parts.append(&mut images);
            }
            _ => unreachable!("only text content was selected"),
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
            if let Some(mime) = mime.filter(|_| {
                fields.contains_key("base64")
                    && fields.contains_key("width")
                    && fields.contains_key("height")
            }) {
                let path = fields
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed image");
                if fields.get("truncated").and_then(Value::as_bool) == Some(true) {
                    return Err(format!("image tool result `{path}` is truncated; incomplete image bytes cannot be sent to the model"));
                }
                let prefix = format!("data:{mime};base64,");
                let Some(Value::String(mut encoded)) = fields.remove("base64") else {
                    return Err("image tool result base64 content is not a string".into());
                };
                if encoded.is_empty() {
                    return Err("image tool result has empty base64 content".into());
                }
                encoded.insert_str(0, &prefix);
                images.push(json!({"type": "image_url", "image_url": {"url": encoded}}));
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
