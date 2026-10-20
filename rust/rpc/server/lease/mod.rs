//! `lease/list`, `lease/release` and `lease/end` over Jeden's RPC protocol:
//! the Brama accounts this agent's sessions run on, what each has left, and
//! ending a lease — the same answers `jeden lease list|release|end --json`
//! print, so Jeden Desktop's Brama account screen reads what the CLI reads.
//!
//! `lease/list` answers Brama's register as Jeden's agent sees it (`leases`,
//! `counts`, `limits`, `unstated`) with `capacity`, keyed by subscription id:
//! Brama's `accounts.capacity` row of each leased account. A pool document
//! without that object answers `capacity: null`, which the reader says
//! rather than reading as unlimited.

use serde_json::{json, Value};

use crate::model_router::lease;
use crate::model_router::ChatConfig;
use crate::Args;

const INVALID: &str = "invalid_params";
const REFUSED: &str = "lease_error";

/// Brama as this agent reaches it from `cwd`'s configuration: the same
/// address, bearer and signing secret a session's model calls use. A
/// configuration that does not reach Brama holds no lease, and says why.
fn gateway(params: &Value) -> Result<ChatConfig, (&'static str, String)> {
    let cwd = super::context::cwd(params)?;
    let args = Args {
        command: crate::Command::Lease,
        cwd: cwd.clone(),
        cwd_explicit: true,
        ..Args::default()
    };
    let config = crate::agent::model_router_config(&crate::load_config(&cwd), &args);
    if !config.via_brama {
        return Err((
            REFUSED,
            format!(
                "{} names a provider called without Brama; it keeps no pool of accounts, so no session holds a lease",
                crate::agent::credential::DIRECT_URL
            ),
        ));
    }
    if config.url.trim().is_empty() || config.secret.is_empty() {
        let cause = config.config_error.clone().ok_or((
            REFUSED,
            "the router configuration names no Brama address or signing secret and gives no cause"
                .to_string(),
        ))?;
        return Err((
            REFUSED,
            format!("Brama cannot be reached to read the lease register: {cause}"),
        ));
    }
    Ok(config)
}

pub(super) fn list(params: &Value) -> Result<Value, (&'static str, String)> {
    let config = gateway(params)?;
    let register = lease::register(&config).map_err(|error| (REFUSED, error))?;
    let capacity = lease::capacity(&config, &register.leases).map_err(|error| (REFUSED, error))?;
    let mut answer = serde_json::to_value(&register).map_err(|error| {
        (
            "internal_error",
            format!("the lease register cannot be encoded: {error}"),
        )
    })?;
    answer["capacity"] = json!(capacity);
    Ok(answer)
}

pub(super) fn release(params: &Value) -> Result<Value, (&'static str, String)> {
    let config = gateway(params)?;
    let lease_id = match params.get("leaseId") {
        Some(Value::String(id)) if !id.trim().is_empty() => id.trim(),
        _ => return Err((INVALID, "lease/release requires leaseId".to_string())),
    };
    let released = lease::release(&config, lease_id).map_err(|error| (REFUSED, error))?;
    Ok(json!({ "released": released }))
}

pub(super) fn end(params: &Value) -> Result<Value, (&'static str, String)> {
    let config = gateway(params)?;
    let session_id = match params.get("sessionId") {
        Some(Value::String(id)) if !id.trim().is_empty() => id.trim(),
        _ => return Err((INVALID, "lease/end requires sessionId".to_string())),
    };
    let released = lease::release_session(&config, session_id).map_err(|error| (REFUSED, error))?;
    Ok(json!({ "released": released }))
}
