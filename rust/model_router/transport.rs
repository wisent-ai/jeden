use super::ChatConfig;
use std::borrow::Cow;

/// The endpoint and its bearer are resolved together for one request.
pub(crate) struct RequestRoute<'a> {
    pub(crate) endpoint: Cow<'a, str>,
    pub(crate) bearer: Cow<'a, str>,
}

impl RequestRoute<'_> {
    pub(crate) fn failure(
        &self,
        config: &ChatConfig,
        operation: &str,
        cause: impl std::fmt::Display,
    ) -> String {
        config.endpoint_failure_on(&self.endpoint, &self.bearer, operation, cause)
    }
}

impl ChatConfig {
    /// Only a route explicitly selected from this process's private gateway
    /// follows its replacements. Caller-supplied and stored providers remain
    /// pinned to the endpoint and credentials supplied in their configuration.
    pub(crate) fn request_route(&self) -> Result<RequestRoute<'_>, String> {
        if !self.own_gateway {
            return Ok(RequestRoute {
                endpoint: Cow::Borrowed(&self.url),
                bearer: Cow::Borrowed(&self.bearer_token),
            });
        }
        if self.via_brama {
            return Err("resolve private-gateway model request: configuration also selects shared Brama account leases".to_owned());
        }
        let gateway = crate::agent::credential::private_gateway::start().map_err(|cause| {
            format!("resolve private-gateway model request: {cause}")
        })?;
        Ok(RequestRoute {
            endpoint: Cow::Owned(gateway.endpoint),
            bearer: Cow::Owned(gateway.bearer),
        })
    }

    pub(crate) fn endpoint_failure(
        &self,
        operation: &str,
        cause: impl std::fmt::Display,
    ) -> String {
        self.endpoint_failure_on(&self.url, &self.bearer_token, operation, cause)
    }

    fn endpoint_failure_on(
        &self,
        endpoint: &str,
        bearer: &str,
        operation: &str,
        cause: impl std::fmt::Display,
    ) -> String {
        let mut message = format!(
            "{operation} at {}/v1/chat/completions ({}) failed: {cause}",
            endpoint.trim_end_matches('/'),
            self.endpoint_origin
        );
        if self.endpoint_via_stado {
            match channel_observation(endpoint) {
                Ok(observation) => {
                    message.push_str("; local resolver observation: ");
                    message.push_str(&observation);
                }
                Err(error) => {
                    message.push_str("; local resolver observation failed: ");
                    message.push_str(&error);
                }
            }
        }
        if !bearer.is_empty() && message.contains(bearer) {
            message = message.replace(bearer, "[REDACTED]");
        }
        message
    }
}

fn channel_observation(endpoint: &str) -> Result<String, String> {
    let url = url::Url::parse(endpoint)
        .map_err(|error| format!("cannot parse selected resolver endpoint {endpoint}: {error}"))?;
    let bind = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let operation = format!("stado resolver channels --bind {bind} --json");
    let output = stado_wait::output(
        std::process::Command::new("stado")
            .args(["resolver", "channels", "--bind", bind, "--json"]),
    )
    .map_err(|error| format!("cannot run {operation}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{operation} ended {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let observation: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{operation} returned invalid JSON: {error}"))?;
    if observation["bind"].as_str() != Some(bind)
        || observation["health_observed"].as_bool() != Some(false)
        || !observation["channels"].is_array()
        || !observation["source"].is_string()
    {
        return Err(format!(
            "{operation} returned an invalid channel observation: {observation}"
        ));
    }
    Ok(observation.to_string())
}
