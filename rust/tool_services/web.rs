use super::config;
use super::types::{
    bounded_json, check_operation, nonempty, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use reqwest::blocking::{Client, Response};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use url::Url;

pub(crate) const TOOLS: &[(&str, &str)] = &[(
    "web_search",
    "Search configured web providers with URL-bearing citations and fallback",
)];
#[derive(Clone)]
struct Provider {
    name: &'static str,
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
        if let Some(token) = config::string(
            value,
            &["toolServices", "web", "tavilyApiKey"],
            "TAVILY_API_KEY",
        ) {
            secrets.insert("web.tavily", token.into_bytes());
            providers.push(Provider {
                name: "tavily",
                endpoint: config::string(
                    value,
                    &["toolServices", "web", "tavilyEndpoint"],
                    "JEDEN_TAVILY_URL",
                )
                .unwrap_or_else(|| "https://api.tavily.com/search".into()),
                secret_ref: "web.tavily",
            });
        }
        if let Some(token) = config::string(
            value,
            &["toolServices", "web", "braveApiKey"],
            "BRAVE_SEARCH_API_KEY",
        ) {
            secrets.insert("web.brave", token.into_bytes());
            providers.push(Provider {
                name: "brave",
                endpoint: config::string(
                    value,
                    &["toolServices", "web", "braveEndpoint"],
                    "JEDEN_BRAVE_SEARCH_URL",
                )
                .unwrap_or_else(|| "https://api.search.brave.com/res/v1/web/search".into()),
                secret_ref: "web.brave",
            });
        }
        Self { providers, secrets }
    }
    pub(crate) fn health(&self) -> HealthDescriptor {
        match self.providers.as_slice() {
            [] => HealthDescriptor::unavailable(
                "web",
                "configure TAVILY_API_KEY or BRAVE_SEARCH_API_KEY",
            ),
            [provider] => HealthDescriptor::healthy("web", provider.name),
            providers => HealthDescriptor::healthy(
                "web",
                providers
                    .iter()
                    .map(|p| p.name)
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
            .unwrap_or(8)
            .clamp(1, 20) as usize;
        let mut failures = Vec::new();
        for provider in &self.providers {
            check_operation(context)?;
            match search_provider(provider, &self.secrets, &query, count, context) {
                Ok(results) if !results.is_empty() => {
                    return bounded_json(
                        context,
                        "web",
                        &json!({"ok":true,"provider":provider.name,"query":query,"results":results,"citations":results.iter().enumerate().map(|(index,result)| json!({"id":index + 1,"url":result["url"],"title":result["title"]})).collect::<Vec<_>>() }),
                    )
                }
                Ok(_) => failures.push(format!("{} returned no results", provider.name)),
                Err(error) => failures.push(format!("{}: {}", provider.name, error)),
            }
        }
        Err(ServiceError::Backend {
            service: "web",
            detail: failures.join("; "),
        })
    }
}
fn search_provider(
    provider: &Provider,
    secrets: &crate::tool_runtime::runtime_ops::secrets::SecretBroker,
    query: &str,
    count: usize,
    context: &OperationContext<'_>,
) -> ServiceResult<Vec<Value>> {
    let endpoint = Url::parse(&provider.endpoint)
        .map_err(|e| ServiceError::InvalidInput(format!("invalid provider endpoint: {e}")))?;
    let value: Value = send_provider(provider, secrets, query, count, context, endpoint, 0)?
        .json()
        .map_err(|e| ServiceError::Protocol {
            service: "web",
            detail: e.to_string(),
        })?;
    let raw = if provider.name == "tavily" {
        value.get("results").and_then(Value::as_array)
    } else {
        value.pointer("/web/results").and_then(Value::as_array)
    }
    .cloned()
    .unwrap_or_default();
    Ok(raw.into_iter().take(count).filter_map(|entry| {
        let url = entry.get("url").and_then(Value::as_str)?; let parsed = Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http"|"https") { return None; }
        Some(json!({"title":entry.get("title").and_then(Value::as_str).unwrap_or(url),"url":url,"snippet":entry.get("content").or_else(|| entry.get("description")).and_then(Value::as_str).unwrap_or("")}))
    }).collect())
}

fn send_provider(
    provider: &Provider,
    secrets: &crate::tool_runtime::runtime_ops::secrets::SecretBroker,
    query: &str,
    count: usize,
    context: &OperationContext<'_>,
    url: Url,
    redirects: u8,
) -> ServiceResult<Response> {
    check_operation(context)?;
    let target =
        crate::tool_runtime::runtime_ops::network::authorize_url(context.execution_grant(), &url)
            .map_err(|e| ServiceError::PermissionDenied(e.to_string()))?;
    if redirects > context.execution_grant().network.max_redirects {
        return Err(ServiceError::PermissionDenied(
            "web redirect limit exceeded".into(),
        ));
    }
    let socket = crate::tool_runtime::runtime_ops::network::pinned_socket(&target)
        .map_err(|e| ServiceError::PermissionDenied(e.to_string()))?;
    let timeout = context
        .deadline()
        .and_then(|d| d.checked_duration_since(std::time::Instant::now()))
        .unwrap_or(Duration::from_secs(20))
        .min(Duration::from_secs(30));
    let client = Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&target.host, socket)
        .build()
        .map_err(|e| ServiceError::Backend {
            service: "web",
            detail: e.to_string(),
        })?;
    let response = secrets
        .expose(context.execution_grant(), provider.secret_ref, |token| {
            let token = std::str::from_utf8(token).map_err(|_| {
                ServiceError::PermissionDenied("provider credential is not UTF-8".into())
            })?;
            if provider.name == "tavily" {
                client
                    .post(target.url.clone())
                    .json(&json!({"api_key":token,"query":query,"max_results":count}))
                    .send()
            } else {
                client
                    .get(target.url.clone())
                    .header("X-Subscription-Token", token)
                    .query(&[("q", query), ("count", &count.to_string())])
                    .send()
            }
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
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ServiceError::Protocol {
                service: "web",
                detail: "redirect missing Location".into(),
            })?;
        let next = crate::tool_runtime::runtime_ops::network::validate_redirect(
            context.execution_grant(),
            &target,
            location,
        )
        .map_err(|e| ServiceError::PermissionDenied(e.to_string()))?;
        return send_provider(
            provider,
            secrets,
            query,
            count,
            context,
            next.url,
            redirects.saturating_add(1),
        );
    }
    response
        .error_for_status()
        .map_err(|e| ServiceError::Backend {
            service: "web",
            detail: e.to_string(),
        })
}
