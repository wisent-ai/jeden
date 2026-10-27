use super::*;

mod attempt;
mod request;
mod support;
mod wire;

pub(crate) use attempt::*;
pub(crate) use request::*;
pub(crate) use support::*;
pub(crate) use wire::*;

/// Streaming chat completion. Requests SSE (`stream: true`); for each content
/// delta it calls `on_delta`, and for each reasoning delta `on_reasoning`
/// (reasoning never counts as visible output, so a route may still retry
/// after it). Falls back to whole-body parsing if the endpoint ignores
/// `stream` and returns a normal JSON completion. Tool-call responses are
/// accumulated and returned as an action string (no partial tool deltas are
/// surfaced). Returns the same action/content string as `chat_completion`.
#[allow(clippy::too_many_arguments)]
pub fn chat_completion_streaming(
    config: &ChatConfig,
    messages: Vec<Value>,
    max_tokens: Option<usize>,
    tools: &[Value],
    on_delta: &mut dyn FnMut(&str) -> bool,
    on_reasoning: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<StreamingCompletion, StreamFailure> {
    let refused = |message| stream_failure(StreamErrorClass::Permanent, message, Vec::new(), false);
    let observation = crate::agent::observations::ObservedCall::begin(
        config.session_id.as_deref(),
        None,
        crate::agent::observations::OperationKind::Model,
        "model",
        json!({ "model": config.model, "endpointOrigin": config.endpoint_origin,
            "messages": messages, "maxTokens": max_tokens, "tools": tools, "stream": true }),
    )
    .map_err(refused)?;
    let mut received = String::new();
    let mut visible = false;
    let mut observed_delta = |piece: &str| {
        received.push_str(piece);
        let shown = on_delta(piece);
        visible |= shown;
        shown
    };
    let result = complete_streamed(
        config,
        messages,
        max_tokens,
        tools,
        &mut observed_delta,
        on_reasoning,
        cancelled,
    );
    let outcome = match &result {
        Ok(served) => json!({ "status": "completed", "output": {
            "content": served.completion.content, "route": served.route, "routeResults": served.route_results } }),
        Err(failure) => json!({ "status": "failed", "cause": failure.message,
            "class": failure.class, "routeResults": failure.route_results, "receivedText": received }),
    };
    if let Err(cause) = observation.record_result(outcome) {
        return Err(match result {
            Ok(served) => stream_failure(
                StreamErrorClass::Permanent,
                cause,
                served.route_results,
                visible,
            ),
            Err(mut failure) => {
                failure.message = format!("{}; {cause}", failure.message);
                failure
            }
        });
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn complete_streamed(
    config: &ChatConfig,
    messages: Vec<Value>,
    max_tokens: Option<usize>,
    tools: &[Value],
    on_delta: &mut dyn FnMut(&str) -> bool,
    on_reasoning: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<StreamingCompletion, StreamFailure> {
    if let Some(cause) = crate::agent::credential::direct_refusal() {
        return Err(stream_failure(
            StreamErrorClass::Permanent,
            cause.to_owned(),
            Vec::new(),
            false,
        ));
    }
    if let Some(message) = &config.config_error {
        return Err(stream_failure(
            StreamErrorClass::Permanent,
            message.clone(),
            Vec::new(),
            false,
        ));
    }
    let primary = RouteDescriptor {
        model: if config.model == AUTOMATIC_MODEL_ROUTE && messages_use_image_input(&messages) {
            VISION_MODEL_ROUTE.to_string()
        } else {
            config.model.clone()
        },
        service_tier: nonempty(&config.service_tier),
    };
    let mut routes = vec![primary];
    for route in &config.fallbacks {
        if !routes.contains(route) {
            routes.push(route.clone());
        }
    }

    let attempts = config.retry.max_attempts;
    let mut route_results = Vec::with_capacity(routes.len().saturating_mul(attempts));
    let mut last_error: Option<AttemptError> = None;
    let mut remaining_routes = routes.iter().peekable();
    while let Some(route) = remaining_routes.next() {
        let following_route = remaining_routes.peek().copied();
        if !config.via_brama && is_virtual_model_route(&route.model) {
            let message = format!(
                "{} is a Brama virtual route; choose a concrete direct-provider model with /model",
                route.model
            );
            if let Some(next) = following_route {
                route_results.push(RouteResult::RouteChanged {
                    from: route.clone(),
                    to: next.clone(),
                    reason: message.clone(),
                });
            }
            last_error = Some(AttemptError::permanent(message));
            continue;
        }
        if let Err(message) = ensure_image_capability(config, &route.model, &messages) {
            if let Some(next) = following_route {
                route_results.push(RouteResult::RouteChanged {
                    from: route.clone(),
                    to: next.clone(),
                    reason: message.clone(),
                });
            }
            last_error = Some(AttemptError::permanent(message));
            continue;
        }
        // The account this route runs on, leased from Brama's pool for the
        // session. A lease Brama refuses (a full pool, no usable member) is
        // this route's error, and the next route is tried.
        let mut leased = match lease::for_route(config, &route.model) {
            Ok(leased) => leased,
            Err(message) => {
                if let Some(next) = following_route {
                    route_results.push(RouteResult::RouteChanged {
                        from: route.clone(),
                        to: next.clone(),
                        reason: message.clone(),
                    });
                }
                last_error = Some(AttemptError::permanent(message));
                continue;
            }
        };
        let mut accounts_tried = BTreeSet::new();
        'account: loop {
            if let Some(held) = &leased {
                accounts_tried.insert(held.subscription_id.clone());
            }
            let mut moved = None;
            for _attempt in std::iter::repeat_n((), attempts) {
                if cancelled() {
                    return Err(stream_failure(
                        StreamErrorClass::Cancelled,
                        "Turn cancelled.".into(),
                        route_results,
                        false,
                    ));
                }
                match streaming_attempt(
                    config,
                    route,
                    &messages,
                    max_tokens,
                    tools,
                    leased.as_ref(),
                    on_delta,
                    on_reasoning,
                    cancelled,
                ) {
                    Ok(completion) => {
                        return Ok(StreamingCompletion {
                            completion,
                            route: route.clone(),
                            route_results,
                            lease: leased,
                        });
                    }
                    Err(error) => {
                        // A call on a leased account asks Brama for the
                        // session's account again; Brama passes over an
                        // account it holds a block against or that is at
                        // the operator's limit.
                        if let Some(held) =
                            leased.as_ref().filter(|_| asks_for_another_account(&error))
                        {
                            match lease::switch(config, held) {
                                Ok(Some(next))
                                    if !accounts_tried.contains(&next.subscription_id) =>
                                {
                                    moved = Some((next, error.message));
                                    break;
                                }
                                Ok(_) => {}
                                Err(refused) => {
                                    let mut message = format!("{}; {refused}", error.message);
                                    if let Some(delay) = error.retry_after {
                                        message.push_str(&format!(
                                            "; the provider asked for a wait of {} s on account {}",
                                            delay.as_secs(),
                                            held.member()
                                        ));
                                    }
                                    last_error = Some(AttemptError { message, ..error });
                                    break 'account;
                                }
                            }
                        }
                        if error.class == StreamErrorClass::QuotaExhausted && !error.visible_output
                        {
                            last_error = Some(error);
                            break;
                        }
                        let retryable = error.is_transient() && !error.visible_output;
                        if !retryable {
                            return Err(stream_failure(
                                error.class,
                                error.message,
                                route_results,
                                error.visible_output,
                            ));
                        }
                        // A transient failure moves to the next route at
                        // once; the last one's error is what the turn
                        // reports when none is left.
                        last_error = Some(error);
                        break;
                    }
                }
            }
            let (Some((next, reason)), Some(from)) = (moved, leased.take()) else {
                break 'account;
            };
            route_results.push(RouteResult::SubscriptionChanged {
                from,
                to: next.clone(),
                reason,
            });
            leased = Some(next);
        }
        if let Some(next) = following_route {
            route_results.push(RouteResult::RouteChanged {
                from: route.clone(),
                to: next.clone(),
                reason: "the route's account and transient attempts are exhausted".into(),
            });
        }
    }
    let error = match last_error {
        Some(error) => error,
        None => AttemptError::permanent("no model route configured"),
    };
    Err(stream_failure(
        error.class,
        error.message,
        route_results,
        error.visible_output,
    ))
}

/// Whether a failed call on a leased account asks Brama for the session's
/// account again: nothing was shown yet, and the failure is one an account
/// at its limit answers (a quota refusal, a rate limit, a transient error).
/// Brama decides whether the account is spent; the error's words never do.
fn asks_for_another_account(error: &AttemptError) -> bool {
    !error.visible_output
        && (error.class == StreamErrorClass::QuotaExhausted || error.is_transient())
}
