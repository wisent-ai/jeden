use crate::capability::{self, CapabilityKind, HealthState};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

fn control_plane_probe(
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

fn process_probe(subsystem: &'static str, program: &str, args: &[&str]) -> HealthProbe {
    let started = Instant::now();
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return HealthProbe::unavailable(
                subsystem,
                started,
                format!("cannot start {program}: {error}"),
            )
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return HealthProbe::healthy(
                    subsystem,
                    started,
                    format!("{program} probe exited successfully"),
                    None,
                )
            }
            Ok(Some(status)) => {
                return HealthProbe::unavailable(
                    subsystem,
                    started,
                    format!("{program} probe exited with {status}"),
                )
            }
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return HealthProbe::unavailable(
                    subsystem,
                    started,
                    format!(
                        "{program} probe timed out after {}ms",
                        PROBE_TIMEOUT.as_millis()
                    ),
                );
            }
            Err(error) => {
                let _ = child.kill();
                return HealthProbe::unavailable(
                    subsystem,
                    started,
                    format!("{program} probe failed: {error}"),
                );
            }
        }
    }
}

fn storage_probe(cwd: &Path) -> HealthProbe {
    let started = Instant::now();
    let dir = cwd.join(".jeden");
    if let Err(error) = fs::create_dir_all(&dir) {
        return HealthProbe::unavailable("storage", started, error.to_string());
    }
    let path = dir.join(format!(".doctor-{}", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(b"jeden-health-v1")
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        if bytes != b"jeden-health-v1" {
            return Err("storage read-after-write mismatch".into());
        }
        Ok(())
    })();
    let _ = fs::remove_file(&path);
    match result {
        Ok(()) => HealthProbe::healthy(
            "storage",
            started,
            "durable write/read/remove succeeded",
            None,
        ),
        Err(error) => HealthProbe::unavailable("storage", started, error),
    }
}

fn registry_probe(
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

fn task_probe(cwd: &Path) -> HealthProbe {
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

fn memory_probe() -> HealthProbe {
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

pub fn doctor(cwd: &Path) -> DoctorReport {
    let brama_client = crate::control_plane::brama::BramaClient::from_env();
    let brama = control_plane_probe("brama", brama_client.health(), || {
        brama_client
            .catalog(false)
            .map(|catalog| json!({"version":catalog.version,"models":catalog.models.len()}))
            .map_err(|error| error.to_string())
    });
    let weles_client = crate::control_plane::weles::WelesClient::from_env();
    let weles = control_plane_probe("weles", weles_client.health(), || {
        weles_client
            .providers()
            .map(|providers| json!({"providers":providers.len()}))
            .map_err(|error| error.to_string())
    });
    let storage = storage_probe(cwd);
    let process = process_probe("process", "/bin/sh", &["-c", "exit 0"]);
    let mcp = {
        let started = Instant::now();
        match crate::mcp::manager_status(cwd) {
            Ok(value) => HealthProbe::healthy("mcp", started, "manager synchronized", Some(value)),
            Err(error) => HealthProbe::unavailable("mcp", started, error),
        }
    };
    let extensions = registry_probe(cwd, "extensions", |d| {
        matches!(
            d.kind,
            CapabilityKind::Extension | CapabilityKind::PluginContribution
        )
    });
    let lsp = registry_probe(cwd, "lsp", |d| {
        d.id.contains("lsp") || d.operations.iter().any(|op| op.contains("lsp"))
    });
    let browser = registry_probe(cwd, "browser", |d| {
        d.id.contains("browser")
            || d.metadata.get("service").and_then(Value::as_str) == Some("browser")
    });
    let collab = registry_probe(cwd, "collab", |d| {
        d.id.contains("collab") || d.operations.iter().any(|op| op.contains("collab"))
    });
    let task = task_probe(cwd);
    let memory = memory_probe();
    let probes = vec![
        brama, weles, storage, process, mcp, extensions, lsp, browser, task, memory, collab,
    ];
    let healthy = probes.iter().all(HealthProbe::available);
    DoctorReport {
        schema_version: 1,
        healthy,
        cwd: cwd.to_path_buf(),
        probes,
    }
}
