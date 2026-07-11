use super::config;
use super::types::{bounded_json, check_operation, nonempty, HealthDescriptor, ServiceError, ServiceResult};
use crate::tool_runtime::runtime_ops::OperationContext;
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use url::Url;

pub(crate) const TOOLS: &[(&str, &str)] = &[("web_search", "Search configured web providers with URL-bearing citations and fallback")];
#[derive(Clone)] struct Provider { name: &'static str, endpoint: String, token: String }
pub(crate) struct WebService { providers: Vec<Provider> }
impl WebService {
    pub(crate) fn discover(_cwd: &Path, value: &Value) -> Self {
        let mut providers = Vec::new();
        if let Some(token) = config::string(value, &["toolServices","web","tavilyApiKey"], "TAVILY_API_KEY") { providers.push(Provider { name: "tavily", endpoint: config::string(value, &["toolServices","web","tavilyEndpoint"], "JEDEN_TAVILY_URL").unwrap_or_else(|| "https://api.tavily.com/search".into()), token }); }
        if let Some(token) = config::string(value, &["toolServices","web","braveApiKey"], "BRAVE_SEARCH_API_KEY") { providers.push(Provider { name: "brave", endpoint: config::string(value, &["toolServices","web","braveEndpoint"], "JEDEN_BRAVE_SEARCH_URL").unwrap_or_else(|| "https://api.search.brave.com/res/v1/web/search".into()), token }); }
        Self { providers }
    }
    pub(crate) fn health(&self) -> HealthDescriptor {
        match self.providers.as_slice() {
            [] => HealthDescriptor::unavailable("web", "configure TAVILY_API_KEY or BRAVE_SEARCH_API_KEY"),
            [provider] => HealthDescriptor::healthy("web", provider.name),
            providers => HealthDescriptor::healthy("web", providers.iter().map(|p| p.name).collect::<Vec<_>>().join(",")),
        }
    }
    pub(crate) fn execute(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
        check_operation(context)?;
        if self.providers.is_empty() { return Err(ServiceError::Unavailable { service: "web", detail: self.health().detail }); }
        let query = nonempty(input.get("query"), "query")?;
        let count = input.get("limit").and_then(Value::as_u64).unwrap_or(8).clamp(1, 20) as usize;
        let timeout = context.deadline().and_then(|d| d.checked_duration_since(std::time::Instant::now())).unwrap_or(Duration::from_secs(20)).min(Duration::from_secs(30));
        let client = Client::builder().timeout(timeout).build().map_err(|e| ServiceError::Backend { service: "web", detail: e.to_string() })?;
        let mut failures = Vec::new();
        for provider in &self.providers {
            check_operation(context)?;
            match search_provider(&client, provider, &query, count) {
                Ok(results) if !results.is_empty() => return bounded_json(context, "web", &json!({"ok":true,"provider":provider.name,"query":query,"results":results,"citations":results.iter().enumerate().map(|(index,result)| json!({"id":index + 1,"url":result["url"],"title":result["title"]})).collect::<Vec<_>>() })),
                Ok(_) => failures.push(format!("{} returned no results", provider.name)),
                Err(error) => failures.push(format!("{}: {}", provider.name, error)),
            }
        }
        Err(ServiceError::Backend { service: "web", detail: failures.join("; ") })
    }
}
fn search_provider(client: &Client, provider: &Provider, query: &str, count: usize) -> ServiceResult<Vec<Value>> {
    let value: Value = if provider.name == "tavily" {
        client.post(&provider.endpoint).json(&json!({"api_key":provider.token,"query":query,"max_results":count})).send().and_then(|r| r.error_for_status()).and_then(|r| r.json()).map_err(|e| ServiceError::Backend { service: "web", detail: e.to_string() })?
    } else {
        client.get(&provider.endpoint).header("X-Subscription-Token", &provider.token).query(&[("q",query),("count",&count.to_string())]).send().and_then(|r| r.error_for_status()).and_then(|r| r.json()).map_err(|e| ServiceError::Backend { service: "web", detail: e.to_string() })?
    };
    let raw = if provider.name == "tavily" { value.get("results").and_then(Value::as_array) } else { value.pointer("/web/results").and_then(Value::as_array) }.cloned().unwrap_or_default();
    Ok(raw.into_iter().take(count).filter_map(|entry| {
        let url = entry.get("url").and_then(Value::as_str)?;
        let parsed = Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http"|"https") { return None; }
        Some(json!({"title":entry.get("title").and_then(Value::as_str).unwrap_or(url),"url":url,"snippet":entry.get("content").or_else(|| entry.get("description")).and_then(Value::as_str).unwrap_or("")}))
    }).collect())
}
