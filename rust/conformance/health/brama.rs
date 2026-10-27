//! The model route: does the selected provider's catalog contain the configured
//! model, and does Brama report readiness when it is the selected transport?
use super::{elapsed, HealthProbe, ProbeState};
use serde_json::json;
use std::path::Path;
use std::time::Instant;

/// The model this checkout would actually send, chosen by the selector
/// routing uses, without the `--model` flag no diagnostic has.
fn configured_model(cwd: &Path) -> Result<Option<String>, String> {
    let declared = crate::cli::config::load_config(cwd).model;
    Ok(crate::agent::selected_model(None, declared.as_deref())?.map(|model| model.trim().to_string()))
}

/// Probe the selected model route rather than treating a responding service as
/// proof that a prompt can run. A catalog can answer while the selected model
/// is absent, or Brama can list models while its runtime is not ready.
pub(super) fn brama_probe(
    cwd: &Path,
    client: &crate::control_plane::brama::BramaClient,
) -> HealthProbe {
    let started = Instant::now();
    let direct = crate::agent::credential::direct_provider();
    if direct.is_none() {
        if let Some(cause) = crate::agent::credential::direct_refusal() {
            return HealthProbe::unavailable(
                "model",
                started,
                format!("direct provider route refused: {cause}"),
            );
        }
    }
    let is_direct = direct.is_some();
    let subsystem = if is_direct { "model" } else { "brama" };
    let active = match direct {
        Some((endpoint, key)) => {
            crate::control_plane::brama::BramaClient::configured(Some(endpoint), key)
        }
        None => client.clone(),
    };
    let origin = if is_direct {
        "direct provider selected by JEDEN_MODEL_ENDPOINT".to_string()
    } else {
        crate::agent::credential::route_origin()
    };
    let health = active.health();
    if !health.available {
        return HealthProbe::unavailable(
            subsystem,
            started,
            format!("{} ({origin})", health.detail),
        );
    }
    let catalog = match crate::control_plane::model_catalog(cwd, &active, is_direct) {
        Ok(catalog) => catalog,
        Err(error) => {
            return HealthProbe::unavailable(subsystem, started, format!("{error} ({origin})"))
        }
    };
    if is_direct && catalog.degraded {
        return HealthProbe::unavailable(subsystem, started,
            "GET /v1/models at the direct provider failed; only a stale catalog was available, so this route is not qualified");
    }
    let configured = match configured_model(cwd) {
        Ok(model) => model,
        Err(error) => return HealthProbe::unavailable(subsystem, started, error),
    };
    if configured.is_none() {
        return HealthProbe::unavailable(subsystem, started,
            format!("catalog answered through {origin}, but no model is selected; configure JEDEN_MODEL or config.model"));
    }
    let route_error = configured.as_deref().and_then(|model| {
        if crate::model_router::is_virtual_model_route(model) {
            return is_direct.then(|| (model.to_owned(),
                "Brama virtual route cannot be used with a direct provider; choose a concrete provider model".into()));
        }
        catalog.resolve(model).err().map(|error| (model.to_owned(), error.to_string()))
    });
    let readiness = if is_direct {
        None
    } else {
        Some(active.readiness())
    };
    let readiness_evidence = match &readiness {
        Some(Ok(value)) => Some(value),
        _ => None,
    };
    let evidence = json!({
        "health": health,
        "probe": {"version": catalog.version, "catalogRevision": catalog.catalog_revision, "models": catalog.models.len()},
        "configuredModel": configured,
        "route": origin,
        "readiness": readiness_evidence,
    });
    if let Some((model, error)) = route_error {
        return HealthProbe {
            subsystem,
            state: ProbeState::Unavailable,
            active: false,
            latency_ms: elapsed(started),
            detail: format!(
                "configured model `{model}` does not resolve in the selected catalog: {error}"
            ),
            evidence: Some(evidence),
        };
    }
    match readiness {
        None => HealthProbe::healthy(
            subsystem,
            started,
            "direct provider catalog resolves; chat authorization is verified by a real model call",
            Some(evidence),
        ),
        Some(Ok(readiness)) if !readiness.ready => HealthProbe::unavailable(
            subsystem,
            started,
            format!(
                "Brama is not ready: {}",
                if readiness.reason.is_empty() {
                    format!("/readyz answered {}", readiness.status)
                } else {
                    readiness.reason
                }
            ),
        ),
        Some(Ok(readiness)) if readiness.degraded || readiness.operator_action_required => {
            HealthProbe::degraded(subsystem, started, readiness.reason.clone(), Some(evidence))
        }
        Some(Err(error)) => HealthProbe::degraded(
            subsystem,
            started,
            format!("catalog resolves; readiness is unknown: {error}"),
            Some(evidence),
        ),
        Some(Ok(_)) => HealthProbe::healthy(
            subsystem,
            started,
            "catalog resolves and Brama reports itself ready",
            Some(evidence),
        ),
    }
}
