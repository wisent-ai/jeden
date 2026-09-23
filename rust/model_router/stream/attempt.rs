use super::*;

#[derive(Debug)]
pub(crate) struct AttemptError {
    pub(crate) class: StreamErrorClass,
    pub(crate) message: String,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) visible_output: bool,
}

impl AttemptError {
    pub(crate) fn permanent(message: impl Into<String>) -> Self {
        Self {
            class: StreamErrorClass::Permanent,
            message: message.into(),
            retry_after: None,
            visible_output: false,
        }
    }

    pub(crate) fn is_transient(&self) -> bool {
        matches!(
            self.class,
            StreamErrorClass::TransientHttp
                | StreamErrorClass::Network
                | StreamErrorClass::EmptyResponse
        )
    }
}

pub(crate) fn stream_failure(
    class: StreamErrorClass,
    message: String,
    route_results: Vec<RouteResult>,
    visible_output: bool,
) -> StreamFailure {
    StreamFailure {
        class,
        message,
        route_results,
        visible_output,
    }
}

pub(crate) fn messages_use_image_input(messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
            })
    })
}

pub(crate) fn ensure_image_capability(
    config: &ChatConfig,
    model: &str,
    messages: &[Value],
) -> Result<(), String> {
    if messages_use_image_input(messages)
        && model != VISION_MODEL_ROUTE
        && !config.image_capable_models.contains(model)
    {
        return Err(format!(
            "model `{model}` does not advertise image input support; choose an image-capable model"
        ));
    }
    Ok(())
}

pub(crate) enum WireMessage {
    Headers {
        status: u16,
        content_type: String,
        retry_after: Option<Duration>,
    },
    FullBody(Result<String, String>),
    Line(Result<String, String>),
    Eof,
    Network(String),
}

pub(crate) fn build_streaming_body(
    route: &RouteDescriptor,
    messages: &[Value],
    max_tokens: Option<usize>,
    tools: &[Value],
    target: Option<&crate::routing::SubscriptionTarget>,
    decision: Option<&crate::routing::RouteDecisionV2>,
) -> Result<Value, String> {
    let mut body = json!({
        "model": route.model,
        "messages": messages,
    });
    if let Some(max_tokens) = max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(service_tier) = route.service_tier.as_ref() {
        body["service_tier"] = json!(service_tier);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    if let Some(target) = target {
        body["billingTarget"] = serde_json::to_value(target).map_err(|error| error.to_string())?;
    }
    if let Some(decision) = decision {
        body["subscriptionDecisionId"] = Value::String(decision.decision_id.clone());
        body["requestId"] = Value::String(decision.request_id.clone());
        body["idempotencyKey"] = Value::String(decision.idempotency_key.clone());
    }
    Ok(body)
}

// Six of these become the request body and the last three are the caller's
// live delta sinks and cancel probe, so no one struct owns the set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn streaming_attempt(
    config: &ChatConfig,
    route: &RouteDescriptor,
    messages: &[Value],
    max_tokens: Option<usize>,
    tools: &[Value],
    target: Option<&crate::routing::SubscriptionTarget>,
    decision: Option<&crate::routing::RouteDecisionV2>,
    on_delta: &mut dyn FnMut(&str) -> bool,
    on_reasoning: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<Completion, AttemptError> {
    let reservation = crate::autonomy::requests::budget::reserve(config, &route.model, max_tokens)
        .map_err(AttemptError::permanent)?;
    let completion = streaming_attempt_inner(
        config,
        route,
        messages,
        max_tokens,
        tools,
        target,
        decision,
        on_delta,
        on_reasoning,
        cancelled,
    )?;
    crate::autonomy::requests::budget::settle(reservation, completion.usage.as_ref())
        .map_err(AttemptError::permanent)?;
    Ok(completion)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn streaming_attempt_inner(
    config: &ChatConfig,
    route: &RouteDescriptor,
    messages: &[Value],
    max_tokens: Option<usize>,
    tools: &[Value],
    target: Option<&crate::routing::SubscriptionTarget>,
    decision: Option<&crate::routing::RouteDecisionV2>,
    on_delta: &mut dyn FnMut(&str) -> bool,
    on_reasoning: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<Completion, AttemptError> {
    let body = build_streaming_body(route, messages, max_tokens, tools, target, decision)
        .map_err(AttemptError::permanent)?;
    let body_text =
        serde_json::to_string(&body).map_err(|error| AttemptError::permanent(error.to_string()))?;
    let (sender, receiver) = mpsc::sync_channel(16);
    spawn_openai_stream_adapter(config, body_text, sender)?;

    let mut state = OpenAiStreamState::default();
    let mut content_type = String::new();
    loop {
        let message = recv_until(&receiver, cancelled).map_err(|class| AttemptError {
            class,
            message: match class {
                StreamErrorClass::Cancelled => "Turn cancelled.".into(),
                StreamErrorClass::Network => "model stream adapter disconnected".into(),
                _ => "model stream receive failure".into(),
            },
            retry_after: None,
            visible_output: state.visible_output,
        })?;
        match message {
            WireMessage::Headers {
                status,
                content_type: kind,
                retry_after,
            } => {
                content_type = kind;
                if !(200..300).contains(&status) {
                    let body = match recv_until(&receiver, cancelled) {
                        Ok(WireMessage::FullBody(Ok(body))) => body,
                        Ok(WireMessage::FullBody(Err(error))) | Ok(WireMessage::Network(error)) => {
                            error
                        }
                        _ => String::new(),
                    };
                    return Err(http_error(status, body, retry_after));
                }
            }
            WireMessage::FullBody(result) => {
                let text = result.map_err(|message| AttemptError {
                    class: StreamErrorClass::Network,
                    message,
                    retry_after: None,
                    visible_output: false,
                })?;
                if content_type.contains("event-stream") {
                    return Err(malformed(
                        "event-stream response arrived without SSE framing",
                        false,
                    ));
                }
                return parse_completion_response(&text).map_err(|message| {
                    if message == NO_MESSAGE || message == NO_MESSAGE_CONTENT {
                        empty_response(message, false)
                    } else {
                        malformed(message, false)
                    }
                });
            }
            WireMessage::Line(result) => {
                let line = result.map_err(|message| AttemptError {
                    class: StreamErrorClass::Network,
                    message,
                    retry_after: None,
                    visible_output: state.visible_output,
                })?;
                if let Some(payload) = state
                    .sse
                    .push_line(&line)
                    .map_err(|message| malformed(message, state.visible_output))?
                {
                    if state.apply_payload(&payload, on_delta, on_reasoning)? {
                        return state.finish();
                    }
                }
            }
            WireMessage::Eof => {
                if let Some(payload) = state
                    .sse
                    .finish()
                    .map_err(|message| malformed(message, state.visible_output))?
                {
                    if state.apply_payload(&payload, on_delta, on_reasoning)? {
                        return state.finish();
                    }
                }
                return Err(AttemptError {
                    class: StreamErrorClass::Network,
                    message: "model stream ended before [DONE]".into(),
                    retry_after: None,
                    visible_output: state.visible_output,
                });
            }
            WireMessage::Network(message) => {
                return Err(AttemptError {
                    class: StreamErrorClass::Network,
                    message,
                    retry_after: None,
                    visible_output: state.visible_output,
                });
            }
        }
    }
}
