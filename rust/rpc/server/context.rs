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

/// The workspace an RPC request reads from: its `cwd`. A request that names
/// none is refused: this server's own working directory is wherever its
/// client launched it, not the project the client is showing, so answering
/// from it read another project's configuration and lease register.
pub(super) fn cwd(params: &Value) -> Result<PathBuf, (&'static str, String)> {
    match params.get("cwd") {
        Some(Value::String(path)) if !path.is_empty() => Ok(PathBuf::from(path)),
        Some(other) => Err((INVALID, format!("cwd must be a non-empty path string; got {other}"))),
        None => Err((INVALID, "the request names no cwd, the workspace it reads from".to_string())),
    }
}

pub(super) fn recommend(
    state: &super::ServerState,
    wire: &super::WireRequest,
) -> Result<Value, (&'static str, String)> {
    let cancellation = state.context_requests.token(&wire.id)?;
    let params = &wire.params;
    let query = match params.get("query") {
        Some(Value::String(query)) if !query.trim().is_empty() => query.trim(),
        Some(Value::String(_)) | None => {
            return Err((INVALID, "context/recommend requires a query".to_string()))
        }
        Some(other) => {
            return Err((
                INVALID,
                format!("context/recommend query must be a string; got {other}"),
            ))
        }
    };
    let cwd = cwd(params)?;
    let config = crate::load_config(&cwd);
    let settings = advisor::settings(&cwd, &config);
    let stated = match params.get("limit") {
        None => None,
        Some(limit) => {
            let whole = limit.as_u64().ok_or_else(|| {
                (
                    INVALID,
                    format!("context/recommend limit must be a whole number; got {limit}"),
                )
            })?;
            let count = usize::try_from(whole).map_err(|error| {
                (
                    INVALID,
                    format!("context/recommend limit {whole} cannot be held here: {error}"),
                )
            })?;
            Some(count)
        }
    };
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
    let cwd = cwd(&wire.params)?;
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
