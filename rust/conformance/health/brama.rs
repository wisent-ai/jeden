//! The model route: does the model this checkout would send resolve in the
//! catalog Brama serves it, and is Brama ready to take it.
use super::{elapsed, HealthProbe, ProbeState};
use serde_json::json;
use std::path::Path;
use std::time::Instant;

/// The model this checkout would actually send, in the precedence
/// `model_router_config` uses minus the `--model` flag no diagnostic has: the
/// merged config first, then `JEDEN_MODEL`.
fn configured_model(cwd: &Path) -> Option<String> {
    crate::cli::config::load_config(cwd)
        .model
        .or_else(|| std::env::var("JEDEN_MODEL").ok())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

/// Brama, judged on what a prompt needs rather than on whether the service
/// answers. Three things can be true at once and only the first was ever
/// reported: the catalog lists, the credentials behind it are refused, and the
/// model this checkout is configured to send is not in the catalog at all. The
/// first was `healthy`, which is how a doctor said fine for a day while every
/// `jeden run` died — the failure this pack has a written rule about.
pub(super) fn brama_probe(
    cwd: &Path,
    client: &crate::control_plane::brama::BramaClient,
) -> HealthProbe {
    let started = Instant::now();
    let health = client.health();
    if !health.available {
        return HealthProbe::unavailable("brama", started, health.detail.clone());
    }
    let catalog = match client.catalog(false) {
        Ok(catalog) => catalog,
        Err(error) => return HealthProbe::unavailable("brama", started, error.to_string()),
    };
    let readiness = client.readiness();
    let configured = configured_model(cwd);
    // The same rule the run loop applies in `model_router_config`: a virtual
    // route resolves itself, everything else has to resolve against the catalog,
    // and `resolve` is what says why when it does not.
    let route_error = configured.as_deref().and_then(|model| {
        if crate::model_router::is_virtual_model_route(model) {
            return None;
        }
        catalog
            .resolve(model)
            .err()
            .map(|error| (model.to_owned(), error.to_string()))
    });
    let evidence = json!({
        "health": health,
        "probe": {"version": catalog.version, "models": catalog.models.len()},
        "configuredModel": configured,
        "readiness": readiness.as_ref().ok(),
    });
    // Fatal, not degraded: `available()` is what the report's verdict and the
    // exit code read, and a model that does not resolve fails every prompt this
    // checkout can send. `HealthProbe::unavailable` carries no evidence, so the
    // state is set here to keep the catalog and the readiness answer attached.
    if let Some((model, error)) = route_error {
        return HealthProbe {
            subsystem: "brama",
            state: ProbeState::Unavailable,
            active: false,
            latency_ms: elapsed(started),
            detail: format!(
                "configured model `{model}` does not resolve in the catalog Brama serves this agent ({} route(s)): {error}",
                catalog.models.len()
            ),
            evidence: Some(evidence),
        };
    }
    match readiness {
        Ok(readiness) if !readiness.ready => HealthProbe::unavailable(
            "brama",
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
        Ok(readiness) if readiness.degraded || readiness.operator_action_required => {
            HealthProbe::degraded("brama", started, readiness.reason.clone(), Some(evidence))
        }
        // A Brama too old to serve `/readyz` leaves the catalog as the only
        // evidence there is, and says so rather than borrowing its verdict.
        Err(error) => HealthProbe::degraded(
            "brama",
            started,
            format!(
                "catalog lists {} route(s); readiness is unknown: {error}",
                catalog.models.len()
            ),
            Some(evidence),
        ),
        Ok(_) => HealthProbe::healthy(
            "brama",
            started,
            "catalog resolves and Brama reports itself ready",
            Some(evidence),
        ),
    }
}
