//! `jeden doctor`: one typed probe per subsystem, and the verdict they add up
//! to. The probes live beside this file by what they examine - the model
//! route (`brama`), the host itself (`host`), the capability registry
//! (`registry`) and the runtimes Jeden drives (`runtime`).
mod brama;
mod host;
mod registry;
mod runtime;

use crate::capability::CapabilityKind;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthProbe {
    pub subsystem: &'static str,
    pub state: ProbeState,
    pub active: bool,
    pub latency_ms: u64,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

impl HealthProbe {
    fn healthy(
        subsystem: &'static str,
        started: Instant,
        detail: impl Into<String>,
        evidence: Option<Value>,
    ) -> Self {
        Self {
            subsystem,
            state: ProbeState::Healthy,
            active: true,
            latency_ms: elapsed(started),
            detail: detail.into(),
            evidence,
        }
    }
    fn degraded(
        subsystem: &'static str,
        started: Instant,
        detail: impl Into<String>,
        evidence: Option<Value>,
    ) -> Self {
        Self {
            subsystem,
            state: ProbeState::Degraded,
            active: true,
            latency_ms: elapsed(started),
            detail: detail.into(),
            evidence,
        }
    }
    fn unavailable(subsystem: &'static str, started: Instant, detail: impl Into<String>) -> Self {
        Self {
            subsystem,
            state: ProbeState::Unavailable,
            active: true,
            latency_ms: elapsed(started),
            detail: detail.into(),
            evidence: None,
        }
    }
    fn available(&self) -> bool {
        self.state != ProbeState::Unavailable
    }
}
fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub schema_version: u32,
    pub healthy: bool,
    pub cwd: PathBuf,
    pub probes: Vec<HealthProbe>,
}

pub fn doctor(cwd: &Path) -> DoctorReport {
    let brama_client = crate::control_plane::brama::BramaClient::from_env();
    let brama = brama::brama_probe(cwd, &brama_client);
    let weles_client = crate::control_plane::weles::WelesClient::from_env();
    let weles = runtime::control_plane_probe("weles", weles_client.health(), || {
        weles_client
            .providers()
            .map(|providers| serde_json::json!({"providers":providers.len()}))
            .map_err(|error| error.to_string())
    });
    let storage = host::storage_probe(cwd);
    let process = host::process_probe("process", "/bin/sh", &["-c", "exit 0"]);
    let sandbox = host::sandbox_probe();
    let mcp = runtime::mcp_probe(cwd);
    let extensions = registry::registry_probe(cwd, "extensions", |d| {
        matches!(
            d.kind,
            CapabilityKind::Extension | CapabilityKind::PluginContribution
        )
    });
    let lsp = registry::registry_probe(cwd, "lsp", |d| {
        d.id.contains("lsp") || d.operations.iter().any(|op| op.contains("lsp"))
    });
    let browser = registry::registry_probe(cwd, "browser", |d| {
        d.id.contains("browser")
            || d.metadata.get("service").and_then(Value::as_str) == Some("browser")
    });
    let collab = registry::registry_probe(cwd, "collab", |d| {
        d.id.contains("collab") || d.operations.iter().any(|op| op.contains("collab"))
    });
    let keymap = registry::keymap_probe(cwd);
    let task = runtime::task_probe(cwd);
    let memory = runtime::memory_probe();
    let probes = vec![
        brama, weles, storage, process, sandbox, mcp, extensions, lsp, browser, task, memory,
        collab, keymap,
    ];
    let healthy = probes.iter().all(HealthProbe::available);
    DoctorReport {
        schema_version: 1,
        healthy,
        cwd: cwd.to_path_buf(),
        probes,
    }
}
