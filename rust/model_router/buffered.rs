//! One model call answered in one body: compaction and classification.
//!
//! A session's call runs on its Brama lease, the account its streamed steps
//! use, and a call refused on that account asks Brama for the session's
//! account again exactly as a streamed step does (`lease::switch`): Brama
//! moves the session only to an account with capacity, and the same account
//! back leaves the refusal as the call's error.

use super::*;

pub fn chat_completion(
    config: &ChatConfig,
    messages: Vec<Value>,
    max_tokens: Option<usize>,
    tools: &[Value],
) -> Result<Completion, String> {
    if let Some(cause) = crate::agent::credential::direct_refusal() {
        return Err(cause.to_owned());
    }
    if let Some(message) = &config.config_error {
        return Err(message.clone());
    }
    if !config.via_brama && is_virtual_model_route(&config.model) {
        return Err(format!(
            "{} is a Brama virtual route; choose a concrete direct-provider model with /model",
            config.model
        ));
    }
    ensure_image_capability(config, &config.model, &messages)?;
    let mut body = json!({
        "model": config.model,
        "messages": messages,
    });
    if let Some(max_tokens) = max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if !config.service_tier.trim().is_empty() {
        body["service_tier"] = Value::String(config.service_tier.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = Value::String("auto".into());
    }
    let body_text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let mut leased = lease::for_route(config, &config.model)?;
    let mut accounts_tried = BTreeSet::new();
    loop {
        if let Some(held) = &leased {
            accounts_tried.insert(held.subscription_id.clone());
        }
        let failure = match post(config, &body_text, leased.as_ref(), max_tokens)? {
            Ok(completion) => return Ok(completion),
            Err(failure) => failure,
        };
        let Some(held) = &leased else {
            return Err(failure);
        };
        match lease::switch(config, held) {
            Ok(Some(next)) if !accounts_tried.contains(&next.subscription_id) => {
                leased = Some(next);
            }
            Ok(_) => return Err(failure),
            Err(refused) => return Err(format!("{failure}; {refused}")),
        }
    }
}

/// One POST of the body on `leased`. The outer error ends the call (the
/// request could not be made or answered, or the gateway refused it for a
/// reason no other account changes); the inner one is a refusal an account
/// at its limit gives (a rate limit, a spent plan, a gateway failure), which
/// may move the session to another.
fn post(
    config: &ChatConfig,
    body_text: &str,
    leased: Option<&lease::SessionLease>,
    max_tokens: Option<usize>,
) -> Result<Result<Completion, String>, String> {
    let signed = agent_headers(body_text, &config.agent_id, &config.secret)?;
    let client = crate::net::blocking_builder().build().map_err(|error| {
        config.endpoint_failure(
            "create HTTP client",
            crate::control_plane::transport::describe_reqwest(error),
        )
    })?;
    let reservation =
        crate::autonomy::requests::budget::reserve(config, &config.model, max_tokens)?;
    let mut store = if !config.via_brama && config.bearer_token.is_empty() {
        crate::agent::credential::tokens::Store::open_existing()?
    } else {
        None
    };
    let mut tried = Vec::new();
    let mut selected = match &mut store {
        Some(store) => store.next(&config.url, &tried)?,
        None => None,
    };
    loop {
        let bearer = match &selected {
            Some(token) => token.secret.as_str(),
            None => &config.bearer_token,
        };
        let mut request = client
            .post(format!(
                "{}/v1/chat/completions",
                config.url.trim_end_matches('/')
            ))
            .header("content-type", "application/json");
        if !bearer.is_empty() {
            request = request.bearer_auth(bearer);
        }
        for (name, value) in &signed {
            request = request.header(*name, value.as_str());
        }
        if let Some(held) = leased {
            for (name, value) in held.headers() {
                request = request.header(name, value);
            }
        }
        let response = request
            .body(body_text.to_string())
            .send()
            .map_err(|error| {
                config.endpoint_failure(
                    "POST model request",
                    crate::control_plane::transport::describe_reqwest(error),
                )
            })?;
        let status = response.status();
        let text = response
            .text()
            .map_err(crate::control_plane::transport::describe_reqwest);
        if !status.is_success() {
            let refusal = match &text {
                Ok(text) => {
                    let body: std::borrow::Cow<'_, str> =
                        if bearer.is_empty() || !text.contains(bearer) {
                            std::borrow::Cow::Borrowed(text.as_str())
                        } else {
                            std::borrow::Cow::Owned(text.replace(bearer, "[REDACTED]"))
                        };
                    config.endpoint_failure(
                        "POST model request",
                        format!("HTTP {}: {body}", status.as_u16()),
                    )
                }
                Err(error) => config.endpoint_failure(
                    "POST model request",
                    format!(
                        "HTTP {}: read model refusal body failed: {error}",
                        status.as_u16()
                    ),
                ),
            };
            let key_rejected = matches!(
                status,
                reqwest::StatusCode::UNAUTHORIZED
                    | reqwest::StatusCode::FORBIDDEN
                    | reqwest::StatusCode::PAYMENT_REQUIRED
                    | reqwest::StatusCode::TOO_MANY_REQUESTS
            );
            if key_rejected {
                if let (Some(token), Some(store)) = (&selected, &mut store) {
                    store.failure(&token.id, &token.secret, "POST model request", &refusal)?;
                    tried.push(token.id.clone());
                    if let Some(next) = store.next(&config.url, &tried)? {
                        selected = Some(next);
                        continue;
                    }
                }
            }
            let account_may_be_spent = status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::PAYMENT_REQUIRED
                || status.is_server_error();
            return if account_may_be_spent {
                Ok(Err(refusal))
            } else {
                Err(refusal)
            };
        }
        let text =
            text.map_err(|error| config.endpoint_failure("read model response body", error))?;
        let safe: std::borrow::Cow<'_, str> = if bearer.is_empty() || !text.contains(bearer) {
            std::borrow::Cow::Borrowed(text.as_str())
        } else {
            std::borrow::Cow::Owned(text.replace(bearer, "[REDACTED]"))
        };
        let completion = parse_completion_response(&safe)?;
        crate::autonomy::requests::budget::settle(reservation, completion.usage.as_ref())?;
        return Ok(Ok(completion));
    }
}
