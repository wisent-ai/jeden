//! `context/recommend` and `context/sources` over Jeden's RPC protocol.
//!
//! Jeden Desktop is an RPC client, not a CLI wrapper, so the advisor reaches
//! the graphical surface the same way configuration does: one operation per
//! question, answering the object the CLI prints with `--json`.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::context::advisor;

const INVALID: &str = "invalid_params";

fn cwd(params: &Value) -> PathBuf {
    params
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

pub(super) fn recommend(params: &Value) -> Result<Value, (&'static str, String)> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if query.is_empty() {
        return Err((INVALID, "context/recommend requires a query".to_string()));
    }
    let cwd = cwd(params);
    let config = crate::load_config(&cwd);
    let settings = advisor::settings(&cwd, &config);
    let mut request = advisor::Request::from_settings(query, &settings);
    if let Some(limit) = params.get("limit").and_then(Value::as_u64) {
        request.limit = advisor::bounded_limit(limit as usize);
    }
    if let Some(declared) = params.get("sources").and_then(Value::as_str) {
        let unknown = advisor::unknown_sources(declared);
        if !unknown.is_empty() {
            return Err((
                INVALID,
                format!(
                    "unknown source(s): {}. Known sources: {}",
                    unknown.join(", "),
                    advisor::SOURCES.join(", ")
                ),
            ));
        }
        request.sources = advisor::parse_sources(declared);
    }
    if let Some(timeout_ms) = params.get("timeoutMs").and_then(Value::as_u64) {
        request.timeout = Duration::from_millis(advisor::bounded_timeout_ms(timeout_ms));
    }
    if request.sources.is_empty() {
        return Err((
            INVALID,
            "no source selected: context.advisor.sources resolved to nothing".to_string(),
        ));
    }
    let advice = advisor::recommend(&cwd, &config, &request);
    serde_json::to_value(&advice).map_err(|error| ("internal_error", error.to_string()))
}

pub(super) fn sources(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = cwd(params);
    let report = advisor::sources_report(&cwd, &crate::load_config(&cwd));
    Ok(json!({
        "cwd": cwd.display().to_string(),
        "settings": report.get("settings").cloned().unwrap_or(Value::Null),
        "selected": report.get("selected").cloned().unwrap_or(Value::Null),
        "sources": report.get("sources").cloned().unwrap_or(Value::Null),
    }))
}
