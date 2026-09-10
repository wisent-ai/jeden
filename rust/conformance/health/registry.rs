//! The capability registry: whether the handlers discovered for this checkout
//! are healthy, and whether the TUI keymap they contribute to is consistent.
use super::HealthProbe;
use crate::capability::{self, HealthState};
use serde_json::json;
use std::path::Path;
use std::time::Instant;

pub(super) fn registry_probe(
    cwd: &Path,
    subsystem: &'static str,
    predicate: impl Fn(&crate::capability::CapabilityDescriptor) -> bool,
) -> HealthProbe {
    let started = Instant::now();
    let snapshot = capability::for_cwd(cwd);
    let matched = snapshot
        .descriptors
        .iter()
        .filter(|descriptor| predicate(descriptor))
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return HealthProbe::degraded(
            subsystem,
            started,
            "no configured capability was discovered",
            Some(json!({"descriptors":[]})),
        );
    }
    let unavailable = matched
        .iter()
        .filter(|descriptor| matches!(descriptor.health.state, HealthState::Unavailable))
        .map(|descriptor| descriptor.id.clone())
        .collect::<Vec<_>>();
    let evidence = Some(
        json!({"descriptors": matched.iter().map(|descriptor| descriptor.id.as_str()).collect::<Vec<_>>()}),
    );
    if unavailable.is_empty() {
        HealthProbe::healthy(
            subsystem,
            started,
            "registered handlers are healthy",
            evidence,
        )
    } else {
        HealthProbe::unavailable(
            subsystem,
            started,
            format!("unavailable handlers: {}", unavailable.join(", ")),
        )
    }
}

pub(super) fn keymap_probe(cwd: &Path) -> HealthProbe {
    let started = Instant::now();
    let snapshot = capability::for_cwd(cwd);
    let Some(descriptor) = snapshot.get("service/tui-keymap") else {
        return HealthProbe::degraded(
            "tui-keymap",
            started,
            "TUI keymap capability was not discovered",
            None,
        );
    };
    let evidence = Some(json!({
        "descriptor": descriptor.id,
        "bindings": descriptor.metadata.get("bindings"),
        "conflicts": descriptor.metadata.get("conflicts"),
        "contextsMutuallyExclusive": descriptor.metadata.get("contextsMutuallyExclusive"),
    }));
    match descriptor.health.state {
        HealthState::Healthy => HealthProbe::healthy(
            "tui-keymap",
            started,
            "namespaced key bindings have no active-context conflicts",
            evidence,
        ),
        HealthState::Degraded => HealthProbe::degraded(
            "tui-keymap",
            started,
            descriptor
                .health
                .detail
                .as_deref()
                .unwrap_or("TUI keymap conflicts detected"),
            evidence,
        ),
        HealthState::Unavailable | HealthState::Disabled => HealthProbe::unavailable(
            "tui-keymap",
            started,
            descriptor
                .health
                .detail
                .as_deref()
                .unwrap_or("TUI keymap diagnostics unavailable"),
        ),
    }
}
