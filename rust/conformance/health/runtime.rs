//! The runtimes Jeden drives: a typed control plane, the MCP manager, the
//! task scheduler and the memory store.
use super::{elapsed, HealthProbe, ProbeState};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(super) fn control_plane_probe(
    subsystem: &'static str,
    health: crate::control_plane::ServiceHealth,
    active: impl FnOnce() -> Result<Value, String>,
) -> HealthProbe {
    let started = Instant::now();
    if !health.available {
        return HealthProbe {
            subsystem,
            state: ProbeState::Unavailable,
            active: false,
            latency_ms: elapsed(started),
            detail: health.detail.clone(),
            evidence: serde_json::to_value(health).ok(),
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

pub(super) fn task_probe(cwd: &Path) -> HealthProbe {
    let started = Instant::now();
    let store = std::env::var_os("JEDEN_TASK_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".jeden/tasks"));
    match crate::task_runtime::TaskScheduler::open(
        cwd,
        &store,
        crate::task_runtime::limits_from_config(cwd),
    ) {
        Ok(scheduler) => {
            let health = scheduler.health();
            if health.healthy {
                HealthProbe::healthy(
                    "task",
                    started,
                    "scheduler store, recovery, and discovery succeeded",
                    serde_json::to_value(health).ok(),
                )
            } else {
                HealthProbe::unavailable("task", started, health.errors.join("; "))
            }
        }
        Err(error) => HealthProbe::unavailable("task", started, error.to_string()),
    }
}

pub(super) fn memory_probe() -> HealthProbe {
    let started = Instant::now();
    match crate::memory::MemoryStore::open(crate::memory::MemoryStore::default_path())
        .and_then(|store| store.health())
    {
        Ok(health) => HealthProbe::healthy(
            "memory",
            started,
            "memory schema and queries succeeded",
            Some(health),
        ),
        Err(error) => HealthProbe::unavailable("memory", started, error),
    }
}
