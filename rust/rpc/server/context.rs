//! `context/recommend` and `context/sources` over Jeden's RPC protocol.
//!
//! Jeden Desktop is an RPC client, not a CLI wrapper, so the advisor reaches
//! the graphical surface the same way configuration does: one operation per
//! question, answering the object the CLI prints with `--json`.

mod requests;
pub(super) use requests::{Lease, Requests};

use std::path::PathBuf;

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

pub(super) fn recommend(
    state: &super::ServerState,
    wire: &super::WireRequest,
) -> Result<Value, (&'static str, String)> {
    let cancellation = state.context_requests.token(&wire.id)?;
    let params = &wire.params;
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
    let stated = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|limit| limit as usize);
    let mut request =
        advisor::Request::new(query, stated, &settings).map_err(|refusal| (INVALID, refusal))?;
    request.cancellation = cancellation;
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
    if request.sources.is_empty() {
        return Err((
            INVALID,
            "no source selected: context.advisor.sources resolved to nothing".to_string(),
        ));
    }
    let advice = advisor::recommend(&cwd, &config, &request);
    serde_json::to_value(&advice).map_err(|error| ("internal_error", error.to_string()))
}

pub(super) fn sources(
    state: &super::ServerState,
    wire: &super::WireRequest,
) -> Result<Value, (&'static str, String)> {
    let cancellation = state.context_requests.token(&wire.id)?;
    let cwd = cwd(&wire.params);
    let mut report = advisor::sources_report(&cwd, &crate::load_config(&cwd), &cancellation);
    report["cwd"] = json!(cwd.display().to_string());
    Ok(report)
}

pub(super) fn begin(
    state: &super::ServerState,
    wire: &super::WireRequest,
) -> Result<Option<Lease>, String> {
    match state.context_requests.begin(&wire.id) {
        Ok(lease) => Ok(Some(lease)),
        Err((code, detail)) => {
            state
                .writer
                .send(&super::error_response(wire.id.clone(), code, &detail))?;
            Ok(None)
        }
    }
}

pub(super) fn cancel(
    state: &super::ServerState,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let id = params
        .get("requestId")
        .ok_or((INVALID, "context/cancel requires requestId".to_string()))?;
    state.context_requests.token(id)?.cancel();
    Ok(json!({"requestId": id, "cancellationRequested": true}))
}
