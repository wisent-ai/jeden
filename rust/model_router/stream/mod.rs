use super::*;

mod attempt;
mod support;
mod wire;

pub(crate) use attempt::*;
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
    for (route_index, route) in routes.iter().enumerate() {
        if let Err(message) = ensure_image_capability(config, &route.model, &messages) {
            if let Some(next) = routes.get(route_index + 1) {
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
                if let Some(next) = routes.get(route_index + 1) {
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
            for _attempt in 1..=attempts {
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
        if let Some(next) = routes.get(route_index + 1) {
            route_results.push(RouteResult::RouteChanged {
                from: route.clone(),
                to: next.clone(),
                reason: "the route's account and transient attempts are exhausted".into(),
            });
        }
    }
    let error = last_error.unwrap_or_else(|| AttemptError::permanent("no model route configured"));
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
