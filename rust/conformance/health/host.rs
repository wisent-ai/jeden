//! The host itself: can Jeden spawn a process, write durably beside the
//! checkout, and enforce the sandbox every task runs inside.
use super::{HealthProbe, PROBE_TIMEOUT};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn process_probe(subsystem: &'static str, program: &str, args: &[&str]) -> HealthProbe {
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

pub(super) fn storage_probe(cwd: &Path) -> HealthProbe {
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

/// Whether `jeden run` can execute a task at all. The scheduler store opens
/// fine on a host whose sandbox helper is missing or unsigned, and every task
/// then dies with "enforced sandbox unavailable" - until now the only way to
/// see that was to run one. On 2026-09-10 a browser service asked forty page
/// questions through such a Jeden and got forty empty answers.
pub(super) fn sandbox_probe() -> HealthProbe {
    let started = Instant::now();
    let health = crate::task_runtime::sandbox::health();
    let evidence = json!({
        "backend": health.backend,
        "enforced": health.enforced,
        "helper": health.helper_path(),
        "executable": std::env::current_exe().ok(),
    });
    if health.enforced {
        HealthProbe::healthy("sandbox", started, health.detail.clone(), Some(evidence))
    } else {
        HealthProbe {
            subsystem: "sandbox",
            state: super::ProbeState::Unavailable,
            active: true,
            latency_ms: super::elapsed(started),
            detail: format!("{}: {}", health.backend, health.detail),
            evidence: Some(evidence),
        }
    }
}
