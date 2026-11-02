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
        let gateway = crate::agent::credential::private_gateway::start()
            .map_err(|cause| format!("resolve private-gateway model request: {cause}"))?;
        Ok(RequestRoute {
            endpoint: Cow::Owned(gateway.endpoint),
            bearer: Cow::Owned(gateway.bearer),
        })
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
        if !bearer.is_empty() && message.contains(bearer) {
            message = message.replace(bearer, "[REDACTED]");
        }
        message
    }
}
