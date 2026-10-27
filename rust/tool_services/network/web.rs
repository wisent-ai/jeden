use super::config;
use super::types::{
    bounded_json, check_operation, nonempty, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tally::FIRST;
use crate::tool_runtime::runtime_ops::OperationContext;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde_json::{json, Value};
use std::path::Path;
use url::Url;

pub(crate) const TOOLS: &[(&str, &str)] = &[(
    "web_search",
    "Search configured web providers with URL-bearing citations and fallback; limit (required) is how many results to answer",
)];

/// The search APIs this service speaks, each with the endpoint its vendor
/// publishes and the shape of its request and answer.
#[derive(Clone, Copy)]
enum Kind {
    /// <https://docs.tavily.com/documentation/api-reference/endpoint/search>
    Tavily,
    /// <https://api-dashboard.search.brave.com/app/documentation/web-search/get-started>
    Brave,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Tavily => "tavily",
            Kind::Brave => "brave",
        }
    }
    fn published_endpoint(self) -> &'static str {
        match self {
            Kind::Tavily => "https://api.tavily.com/search",
            Kind::Brave => "https://api.search.brave.com/res/v1/web/search",
        }
    }
    fn request(self, client: &Client, url: Url, token: &str, query: &str, count: usize) -> RequestBuilder {
        match self {
            Kind::Tavily => client
                .post(url)
                .json(&json!({"api_key": token, "query": query, "max_results": count})),
            Kind::Brave => client
                .get(url)
                .header("X-Subscription-Token", token)
                .query(&[("q", query), ("count", &count.to_string())]),
        }
    }
    /// The results list of an answer, and the field each result's snippet is under.
    fn results(self, answer: &Value) -> Option<(&Vec<Value>, &'static str)> {
        match self {
            Kind::Tavily => answer.get("results").and_then(Value::as_array).map(|list| (list, "content")),
            Kind::Brave => answer.pointer("/web/results").and_then(Value::as_array).map(|list| (list, "description")),
        }
    }
}

#[derive(Clone)]
struct Provider {
    kind: Kind,
    endpoint: String,
    secret_ref: &'static str,
}
pub(crate) struct WebService {
    providers: Vec<Provider>,
    secrets: crate::tool_runtime::runtime_ops::secrets::SecretBroker,
}
impl WebService {
    pub(crate) fn discover(_cwd: &Path, value: &Value) -> Self {
        let mut providers = Vec::new();
        let mut secrets = crate::tool_runtime::runtime_ops::secrets::SecretBroker::default();
        let configured: &[(Kind, &str, &str, &str, &str, &'static str)] = &[
            (Kind::Tavily, "tavilyApiKey", "TAVILY_API_KEY", "tavilyEndpoint", "JEDEN_TAVILY_URL", "web.tavily"),
            (Kind::Brave, "braveApiKey", "BRAVE_SEARCH_API_KEY", "braveEndpoint", "JEDEN_BRAVE_SEARCH_URL", "web.brave"),
        ];
        for &(kind, key_field, key_variable, endpoint_field, endpoint_variable, secret_ref) in configured {
            let Some(token) = config::string(value, &["toolServices", "web", key_field], key_variable) else {
                continue;
            };
            secrets.insert(secret_ref, token.into_bytes());
            // A provider configured without an endpoint override is reached at
            // the endpoint its vendor publishes.
            let endpoint = match config::string(value, &["toolServices", "web", endpoint_field], endpoint_variable) {
                Some(endpoint) => endpoint,
                None => kind.published_endpoint().to_owned(),
            };
            providers.push(Provider { kind, endpoint, secret_ref });
        }
        Self { providers, secrets }
    }
    pub(crate) fn health(&self) -> HealthDescriptor {
        match self.providers.as_slice() {
            [] => HealthDescriptor::unavailable(
                "web",
                "configure TAVILY_API_KEY or BRAVE_SEARCH_API_KEY",
            ),
            providers => HealthDescriptor::healthy(
                "web",
                providers
                    .iter()
                    .map(|p| p.kind.name())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        }
    }
    pub(crate) fn execute(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        check_operation(context)?;
        if self.providers.is_empty() {
            return Err(ServiceError::Unavailable {
                service: "web",
                detail: self.health().detail,
            });
        }
        let query = nonempty(input.get("query"), "query")?;
        let count = input
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(std::num::NonZeroU64::new)
            .ok_or_else(|| {
                ServiceError::InvalidInput(
                    "web search needs limit: how many results the provider returns".into(),
                )
            })?
            .get() as usize;
        let mut failures = Vec::new();
        for provider in &self.providers {
            check_operation(context)?;
            match search_provider(provider, &self.secrets, &query, count, context) {
                Ok((results, unusable)) if !results.is_empty() => {
                    let citations = (FIRST..)
                        .zip(&results)
                        .map(|(id, result)| json!({"id": id, "url": result["url"], "title": result["title"]}))
                        .collect::<Vec<_>>();
                    return bounded_json(
                        context,
                        "web",
                        &json!({"ok": true, "provider": provider.kind.name(), "query": query, "results": results, "citations": citations, "unusable": unusable}),
                    );
                }
                Ok((_, unusable)) => failures.push(format!(
                    "{} returned no usable results{}",
                    provider.kind.name(),
                    unusable.iter().map(|cause| format!("; {cause}")).collect::<String>()
                )),
                Err(error) => failures.push(format!("{}: {}", provider.kind.name(), error)),
            }
        }
        Err(ServiceError::Backend {
            service: "web",
            detail: failures.join("; "),
        })
    }
}

/// The provider's results as `{title, url, snippet}`, and why each result it
/// answered without a URL naming a host was left out.
fn search_provider(
    provider: &Provider,
    secrets: &crate::tool_runtime::runtime_ops::secrets::SecretBroker,
    query: &str,
    count: usize,
    context: &OperationContext<'_>,
) -> ServiceResult<(Vec<Value>, Vec<String>)> {
    let endpoint = Url::parse(&provider.endpoint)
        .map_err(|e| ServiceError::InvalidInput(format!("invalid provider endpoint: {e}")))?;
    let answer: Value = send_provider(provider, secrets, query, count, context, endpoint, Vec::new())?
        .json()
        .map_err(|e| ServiceError::Protocol {
            service: "web",
            detail: e.to_string(),
        })?;
    let (raw, snippet_field) = provider.kind.results(&answer).ok_or_else(|| ServiceError::Protocol {
        service: "web",
        detail: format!("{} answered no results list: {answer}", provider.kind.name()),
    })?;
    let mut results = Vec::new();
    let mut unusable = Vec::new();
    for entry in raw.iter().take(count) {
        let url = match entry.get("url").and_then(Value::as_str).map(|url| (url, Url::parse(url))) {
            Some((url, Ok(parsed))) if parsed.has_host() => url,
            Some((url, Ok(_hostless))) => {
                unusable.push(format!("{url} names no host"));
                continue;
            }
            Some((url, Err(error))) => {
                unusable.push(format!("{url} is not a URL: {error}"));
                continue;
            }
            None => {
                unusable.push(format!("a result without a url: {entry}"));
                continue;
            }
        };
        results.push(json!({"title": entry.get("title"), "url": url, "snippet": entry.get(snippet_field)}));
    }
    Ok((results, unusable))
}

/// One request to the provider, following the redirects the execution grant
/// allows; `followed` holds the redirect targets already taken.
fn send_provider(
    provider: &Provider,
    secrets: &crate::tool_runtime::runtime_ops::secrets::SecretBroker,
    query: &str,
    count: usize,
    context: &OperationContext<'_>,
    url: Url,
    mut followed: Vec<String>,
) -> ServiceResult<Response> {
    check_operation(context)?;
    let target =
        crate::tool_runtime::runtime_ops::network::authorize_url(context.execution_grant(), &url)
            .map_err(|e| ServiceError::PermissionDenied(e.to_string()))?;
    if followed.len() > usize::from(context.execution_grant().network.max_redirects) {
        return Err(ServiceError::PermissionDenied(format!(
            "web redirect limit exceeded: {}",
            followed.join(" -> ")
        )));
    }
    let socket = crate::tool_runtime::runtime_ops::network::pinned_socket(&target)
        .map_err(|e| ServiceError::PermissionDenied(e.to_string()))?;
    let client = crate::net::blocking_builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&target.host, socket)
        .build()
        .map_err(|e| ServiceError::Backend {
            service: "web",
            detail: e.to_string(),
        })?;
    let response = secrets
        .expose(context.execution_grant(), provider.secret_ref, |token| {
            let token = std::str::from_utf8(token).map_err(|error| {
                ServiceError::PermissionDenied(format!("provider credential is not UTF-8: {error}"))
            })?;
            stado_wait::request_blocking(provider.kind.request(&client, target.url.clone(), token, query, count))
                .map_err(|e| ServiceError::Backend {
                    service: "web",
                    detail: e.to_string(),
                })
        })
        .map_err(|e| ServiceError::PermissionDenied(e.to_string()))??;
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .ok_or_else(|| ServiceError::Protocol {
                service: "web",
                detail: "redirect missing Location".into(),
            })?
            .to_str()
            .map_err(|error| ServiceError::Protocol {
                service: "web",
                detail: format!("redirect Location is not text: {error}"),
            })?;
        let next = crate::tool_runtime::runtime_ops::network::validate_redirect(
            context.execution_grant(),
            &target,
            location,
        )
        .map_err(|e| ServiceError::PermissionDenied(e.to_string()))?;
        followed.push(next.url.to_string());
        return send_provider(provider, secrets, query, count, context, next.url, followed);
    }
    response
        .error_for_status()
        .map_err(|e| ServiceError::Backend {
            service: "web",
            detail: e.to_string(),
        })
}
