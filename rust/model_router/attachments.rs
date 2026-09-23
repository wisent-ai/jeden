use super::*;

#[derive(Debug, Clone)]
pub(crate) enum ModelAttachment {
    Image { mime: String, bytes: Arc<[u8]> },
    Text { bytes: Arc<[u8]> },
}

impl ModelAttachment {
    pub(crate) fn image(mime: impl Into<String>, bytes: Arc<[u8]>) -> Result<Self, String> {
        let mime = mime.into();
        if !matches!(
            mime.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return Err(format!("unsupported image attachment MIME type `{mime}`"));
        }
        if bytes.is_empty() {
            return Err("image attachment is empty".into());
        }
        Ok(Self::Image { mime, bytes })
    }

    pub(crate) fn text(bytes: Arc<[u8]>) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("text attachment is empty".into());
        }
        if bytes.len() > MAX_TEXT_ATTACHMENT_BYTES {
            return Err(format!(
                "text attachment is {} bytes; limit is {MAX_TEXT_ATTACHMENT_BYTES}",
                bytes.len()
            ));
        }
        std::str::from_utf8(bytes.as_ref())
            .map_err(|_| "text attachment is not valid UTF-8".to_string())?;
        Ok(Self::Text { bytes })
    }
}

/// Build OpenAI-compatible content parts only in the ephemeral provider copy.
/// The conversation's durable messages remain string-valued.
pub(crate) fn with_attachments(
    mut messages: Vec<Value>,
    attachments: &[ModelAttachment],
) -> Result<Vec<Value>, String> {
    if attachments.is_empty() {
        return Ok(messages);
    }
    let message = messages
        .iter_mut()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .ok_or("cannot attach content: outbound messages contain no user message")?;
    let content = message
        .get_mut("content")
        .ok_or("cannot attach content: latest user message has no content")?;
    let text = match std::mem::take(content) {
        Value::String(text) => text,
        other => {
            *content = other;
            return Err("cannot attach content: latest user message content is not text".into());
        }
    };
    let mut parts = Vec::with_capacity(attachments.len().saturating_add(1));
    parts.push(json!({"type": "text", "text": text}));
    for attachment in attachments {
        match attachment {
            ModelAttachment::Image { mime, bytes } => {
                let encoded_len = (bytes.len().saturating_add(2) / 3).saturating_mul(4);
                let mut url = String::with_capacity(
                    "data:;base64,"
                        .len()
                        .saturating_add(mime.len())
                        .saturating_add(encoded_len),
                );
                url.push_str("data:");
                url.push_str(mime);
                url.push_str(";base64,");
                BASE64.encode_string(bytes.as_ref(), &mut url);
                parts.push(json!({"type": "image_url", "image_url": {"url": url}}));
            }
            ModelAttachment::Text { bytes } => {
                let text = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| "text attachment is not valid UTF-8")?;
                parts.push(json!({"type": "text", "text": text}));
            }
        }
    }
    message["content"] = Value::Array(parts);
    Ok(messages)
}
