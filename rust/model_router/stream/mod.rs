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
    let mut routes = vec![primary.clone()];
    for route in &config.fallbacks {
        if !routes.contains(route) {
            routes.push(route.clone());
        }
    }

    let attempts = config.retry.max_attempts;
    let (request_id, idempotency_key, sticky_key) = logical_request_keys(&primary, &messages);
    let cooldown_store = match config.subscription_cooldown_path.as_ref() {
        Some(path) => Some(
            crate::routing::CooldownStore::open(path).map_err(|message| {
                stream_failure(
                    StreamErrorClass::Permanent,
                    format!("cannot open subscription cooldown store: {message}"),
                    Vec::new(),
                    false,
                )
            })?,
        ),
        None => None,
    };
    let subscription_decision = match config.subscription_pool.as_ref() {
        Some(pool) if !pool.targets.is_empty() => {
            let required = ["chat".to_string()].into_iter().collect();
            let now_ms = epoch_millis();
            Some(
                crate::routing::RouteDecisionV2::freeze(
                    pool,
                    request_id,
                    idempotency_key,
                    &sticky_key,
                    &required,
                    now_ms,
                    |identity| {
                        cooldown_store.as_ref().is_some_and(|store| {
                            store.is_cooling_down(identity, now_ms).unwrap_or(true)
                        })
                    },
                )
                .map_err(|message| {
                    stream_failure(StreamErrorClass::QuotaExhausted, message, Vec::new(), false)
                })?,
            )
        }
        _ => None,
    };
    let max_target_count = subscription_decision
        .as_ref()
        .map_or(1, |decision| decision.targets.len());
    let mut route_results = Vec::with_capacity(
        routes
            .len()
            .saturating_mul(max_target_count)
            .saturating_mul(attempts),
    );
    let mut last_error: Option<AttemptError> = None;
    let mut exhausted_targets = std::collections::BTreeSet::new();
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
        let route_targets =
            subscription_targets_for_route(subscription_decision.as_ref(), &route.model);
        if route_targets.is_empty() {
            last_error = Some(AttemptError {
                class: StreamErrorClass::QuotaExhausted,
                message: format!(
                    "no eligible subscription target for model '{}'",
                    route.model
                ),
                retry_after: None,
                visible_output: false,
            });
            continue;
        }
        for target_index in 0..route_targets.len() {
            let target = route_targets[target_index];
            if target.is_some_and(|target| exhausted_targets.contains(&target.identity())) {
                continue;
            }
            for attempt in 1..=attempts {
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
                    target,
                    target.and(subscription_decision.as_ref()),
                    on_delta,
                    on_reasoning,
                    cancelled,
                ) {
                    Ok(completion) => {
                        return Ok(StreamingCompletion {
                            completion,
                            route: route.clone(),
                            route_results,
                            subscription_target: target.cloned(),
                            subscription_decision_id: target.and(
                                subscription_decision
                                    .as_ref()
                                    .map(|decision| decision.decision_id.clone()),
                            ),
                        });
                    }
                    Err(error) => {
                        if error.class == StreamErrorClass::QuotaExhausted && !error.visible_output
                        {
                            if let Some(target) = target {
                                exhausted_targets.insert(target.identity());
                            }
                            // A cooldown is persisted only for the wait the provider
                            // stated (Retry-After); without one the subscription is
                            // skipped for this turn alone and asked again next turn.
                            if let (Some(store), Some(target), Some(delay)) =
                                (cooldown_store.as_ref(), target, error.retry_after)
                            {
                                let now_ms = epoch_millis();
                                let until_ms = now_ms.saturating_add(duration_millis(delay).max(1));
                                if let Err(message) =
                                    store.record(target.identity(), until_ms, now_ms)
                                {
                                    return Err(stream_failure(
                                        StreamErrorClass::Permanent,
                                        format!("cannot persist subscription cooldown: {message}"),
                                        route_results,
                                        false,
                                    ));
                                }
                            }
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
                        // A transient failure moves to the next subscription or
                        // route at once; the last one's error is what the turn
                        // reports when none is left.
                        last_error = Some(error);
                        break;
                    }
                }
            }
            if let (Some(from), Some(to)) = (
                route_targets.get(target_index).copied().flatten(),
                route_targets.get(target_index + 1).copied().flatten(),
            ) {
                route_results.push(RouteResult::SubscriptionChanged {
                    from: from.clone(),
                    to: to.clone(),
                    reason: "subscription quota or transient attempts exhausted".into(),
                });
            }
        }
        if let Some(next) = routes.get(route_index + 1) {
            route_results.push(RouteResult::RouteChanged {
                from: route.clone(),
                to: next.clone(),
                reason: "subscription targets and transient attempts exhausted".into(),
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
