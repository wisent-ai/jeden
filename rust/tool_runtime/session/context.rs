//! The `context_recommend` tool: the advisor as something the model can ask
//! again mid-task, after the automatic advisory that opened the turn.


use serde_json::{json, Value};

use crate::context::advisor;
use crate::tool_runtime::shared::string_input;
use crate::tool_runtime::ToolRuntime;

pub(crate) fn context_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query")
        .or_else(|| string_input(input, "task"))
        .ok_or("context_recommend requires query")?;
    if query.trim().is_empty() {
        return Err("context_recommend requires a non-empty query".into());
    }
    let config = crate::load_config(runtime.cwd);
    let settings = advisor::settings(runtime.cwd, &config);
    let mut request = advisor::Request::from_settings(&query, &settings);
    if let Some(limit) = input.get("limit").and_then(Value::as_u64) {
        request.limit = advisor::bounded_limit(limit as usize);
    }
    if let Some(declared) = string_input(input, "sources") {
        let unknown = advisor::unknown_sources(&declared);
        if !unknown.is_empty() {
            return Err(format!(
                "unknown source(s): {}. Known sources: {}",
                unknown.join(", "),
                advisor::SOURCES.join(", ")
            ));
        }
        request.sources = advisor::parse_sources(&declared);
    }
    if request.sources.is_empty() {
        return Err("no source selected: context.advisor.sources resolved to nothing".into());
    }
    let advice = advisor::recommend(runtime.cwd, &config, &request);
    let value = serde_json::to_value(&advice).map_err(|error| error.to_string())?;
    Ok(json!({
        "query": advice.query,
        "recommendations": value.get("recommendations").cloned().unwrap_or(json!([])),
        "sources": value.get("sources").cloned().unwrap_or(json!([])),
        "text": advisor::render_text(&advice),
    }))
}
