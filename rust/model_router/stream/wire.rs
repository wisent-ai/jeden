use super::*;

#[derive(Default)]
pub(crate) struct SseDecoder {
    pub(crate) data: String,
}

impl SseDecoder {
    pub(crate) fn push_line(&mut self, line: &str) -> Result<Option<String>, String> {
        if line.is_empty() {
            return self.take_event();
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = match line.split_once(':') {
            Some(parts) => parts,
            None => (line, ""),
        };
        let value = match value.strip_prefix(' ') {
            Some(stripped) => stripped,
            None => value,
        };
        match field {
            "data" => {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            // The other fields the SSE standard defines carry nothing a
            // completion reads.
            "event" => {}
            "id" => {}
            "retry" => {}
            _ => return Err(format!("malformed SSE field: {field}")),
        }
        Ok(None)
    }

    pub(crate) fn finish(&mut self) -> Result<Option<String>, String> {
        self.take_event()
    }

    pub(crate) fn take_event(&mut self) -> Result<Option<String>, String> {
        if self.data.is_empty() {
            return Ok(None);
        }
        Ok(Some(std::mem::take(&mut self.data)))
    }
}

#[derive(Debug)]
pub(crate) enum OpenAiStreamEvent {
    Text(String),
    Thinking(String),
    ToolCalls(Vec<Value>),
    Usage(CompletionUsage),
    Metadata,
    Incomplete,
    Done,
}

#[derive(Default)]
pub(crate) struct OpenAiStreamState {
    pub(crate) sse: SseDecoder,
    pub(crate) content: String,
    pub(crate) tool_calls: Vec<Value>,
    pub(crate) usage: Option<CompletionUsage>,
    pub(crate) visible_output: bool,
}

impl OpenAiStreamState {
    pub(crate) fn apply_payload(
        &mut self,
        payload: &str,
        on_delta: &mut dyn FnMut(&str) -> bool,
        on_reasoning: &mut dyn FnMut(&str),
    ) -> Result<bool, AttemptError> {
        for event in parse_stream_events(payload)
            .map_err(|message| malformed(message, self.visible_output))?
        {
            match event {
                OpenAiStreamEvent::Text(piece) => {
                    self.content.push_str(&piece);
                    self.visible_output |= on_delta(&piece);
                }
                OpenAiStreamEvent::Thinking(piece) => on_reasoning(&piece),
                OpenAiStreamEvent::Metadata => {}
                OpenAiStreamEvent::ToolCalls(calls) => {
                    accumulate_tool_call_deltas(&mut self.tool_calls, &calls)
                        .map_err(|message| malformed(message, self.visible_output))?;
                }
                OpenAiStreamEvent::Usage(usage) => self.usage = Some(usage),
                OpenAiStreamEvent::Incomplete => {
                    return Err(AttemptError {
                        class: StreamErrorClass::Incomplete,
                        message: "model response incomplete: length".into(),
                        retry_after: None,
                        visible_output: self.visible_output,
                    });
                }
                OpenAiStreamEvent::Done => return Ok(true),
            }
        }
        Ok(false)
    }

    pub(crate) fn finish(&mut self) -> Result<Completion, AttemptError> {
        if !self.tool_calls.is_empty() {
            return Ok(Completion {
                content: tool_calls_to_action(&self.tool_calls).map_err(AttemptError::permanent)?,
                usage: self.usage.take(),
            });
        }
        if self.content.trim().is_empty() {
            return Err(empty_response(NO_MESSAGE_CONTENT, self.visible_output));
        }
        Ok(Completion {
            content: std::mem::take(&mut self.content),
            usage: self.usage.take(),
        })
    }
}

pub(crate) fn parse_stream_events(payload: &str) -> Result<Vec<OpenAiStreamEvent>, String> {
    if payload.trim() == "[DONE]" {
        return Ok(vec![OpenAiStreamEvent::Done]);
    }
    let chunk: Value = serde_json::from_str(payload)
        .map_err(|error| format!("malformed model stream JSON: {error}"))?;
    if let Some(error) = chunk.get("error") {
        return Err(format!("model stream error event: {error}"));
    }
    let mut events = Vec::new();
    if let Some(usage) = usage_from_value(&chunk) {
        events.push(OpenAiStreamEvent::Usage(usage));
    }
    let choices = chunk
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("model stream event has no choices array")?;
    if choices.is_empty() {
        if events.is_empty() {
            return Err("model stream event has neither choices nor usage".into());
        }
        return Ok(events);
    }
    let choice = choices
        .first()
        .ok_or("model stream event has no first choice")?;
    if matches!(
        choice.get("finish_reason").and_then(Value::as_str),
        Some("length" | "max_tokens")
    ) {
        events.push(OpenAiStreamEvent::Incomplete);
    }
    let delta = choice
        .get("delta")
        .ok_or("model stream choice has no delta")?;
    let mut recognized = false;
    if let Some(content) = delta.get("content") {
        match content {
            Value::Null => {}
            Value::String(content) if !content.is_empty() => {
                events.push(OpenAiStreamEvent::Text(content.clone()));
            }
            Value::String(_) => {}
            _ => return Err("model stream content delta is not a string or null".into()),
        }
        recognized = true;
    }
    let reasoning = match delta.get("reasoning_content") {
        Some(value) => Some(value),
        None => delta.get("reasoning"),
    };
    if let Some(reasoning) = reasoning {
        match reasoning {
            Value::Null => {}
            Value::String(text) if !text.is_empty() => {
                events.push(OpenAiStreamEvent::Thinking(text.clone()));
            }
            Value::String(_) => {}
            _ => return Err("model stream reasoning delta is not a string or null".into()),
        }
        recognized = true;
    }
    if let Some(calls) = delta.get("tool_calls") {
        events.push(OpenAiStreamEvent::ToolCalls(
            calls
                .as_array()
                .ok_or("model stream tool_calls delta is not an array")?
                .clone(),
        ));
        recognized = true;
    }
    if delta.get("role").is_some() || choice.get("finish_reason").is_some() {
        events.push(OpenAiStreamEvent::Metadata);
        recognized = true;
    }
    if !recognized && events.is_empty() {
        return Err("unrecognized model stream event".into());
    }
    Ok(events)
}
