//! The runtimes Jeden drives: a typed control plane, the MCP manager, the
//! task scheduler and the memory store.
use super::{elapsed, HealthProbe, ProbeState};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

pub(super) fn control_plane_probe(
    subsystem: &'static str,
    health: crate::control_plane::ServiceHealth,
    active: impl FnOnce() -> Result<Value, String>,
) -> HealthProbe {
    let started = Instant::now();
    if !health.available {
        let evidence = match serde_json::to_value(&health) {
            Ok(evidence) => evidence,
            Err(error) => {
                return HealthProbe::unavailable(
                    subsystem,
                    started,
                    format!(
                        "{}; its health cannot be encoded as evidence: {error}",
                        health.detail
                    ),
                );
            }
        };
        return HealthProbe {
            subsystem,
            state: ProbeState::Unavailable,
            active: false,
            latency_ms: elapsed(started),
            detail: health.detail.clone(),
            evidence: Some(evidence),
        };
    }
    match active() {
        Ok(evidence) => HealthProbe::healthy(
            subsystem,
            started,
            "typed control-plane request succeeded",
            Some(json!({"health":health,"probe":evidence})),
        ),
        Err(error) => HealthProbe::unavailable(subsystem, started, error),
    }
}

pub(super) fn mcp_probe(cwd: &Path) -> HealthProbe {
    let started = Instant::now();
    match crate::mcp::manager_status(cwd) {
        Ok(value) => HealthProbe::healthy("mcp", started, "manager synchronized", Some(value)),
        Err(error) => HealthProbe::unavailable("mcp", started, error),
    }
}

/// The task scheduler over the store the task tools of a run without a
/// session directory use (`task_runtime::default_store`), so the probe
/// answers for the store delegation actually opens.
pub(super) fn task_probe(cwd: &Path) -> HealthProbe {
    let started = Instant::now();
    let store = crate::task_runtime::default_store(cwd, None);
    match crate::task_runtime::limits_from_config(cwd)
        .and_then(|limits| crate::task_runtime::TaskScheduler::open(cwd, &store, limits))
    {
        Ok(scheduler) => {
            let health = scheduler.health();
            if !health.healthy {
                return HealthProbe::unavailable("task", started, health.errors.join("; "));
            }
            match serde_json::to_value(&health) {
                Ok(evidence) => HealthProbe::healthy(
                    "task",
                    started,
                    "scheduler store, recovery, and discovery succeeded",
                    Some(evidence),
                ),
                Err(error) => HealthProbe::unavailable(
                    "task",
                    started,
                    format!("the scheduler's health cannot be encoded as evidence: {error}"),
                ),
            }
        }
        Err(error) => HealthProbe::unavailable("task", started, error.to_string()),
    }
}

pub(super) fn memory_probe() -> HealthProbe {
    let started = Instant::now();
    match crate::memory::MemoryStore::open().and_then(|store| store.health()) {
        Ok(health) => HealthProbe::healthy(
            "memory",
            started,
            "memory schema and queries succeeded",
            Some(health),
        ),
        Err(error) => HealthProbe::unavailable("memory", started, error),
    }
}
