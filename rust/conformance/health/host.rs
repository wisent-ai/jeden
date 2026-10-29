//! The host itself: can Jeden spawn a process, write durably beside the
//! checkout, and enforce the sandbox every task runs inside.
use super::HealthProbe;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

pub(super) fn process_probe(subsystem: &'static str, program: &str, args: &[&str]) -> HealthProbe {
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The probe's own process is waited for through the shared wait
    // mechanism, so a probe that hangs says on stderr what it waits for.
    match stado_wait::status(&mut command) {
        Ok(status) if status.success() => HealthProbe::healthy(
            subsystem,
            started,
            format!("{program} probe exited successfully"),
            None,
        ),
        Ok(status) => HealthProbe::unavailable(
            subsystem,
            started,
            format!("{program} probe exited with {status}"),
        ),
        Err(error) => HealthProbe::unavailable(
            subsystem,
            started,
            format!("{program} probe could not be run: {error}"),
        ),
    }
}

pub(super) fn storage_probe(cwd: &Path) -> HealthProbe {
    let started = Instant::now();
    let dir = cwd.join(".jeden");
    if let Err(error) = fs::create_dir_all(&dir) {
        return HealthProbe::unavailable(
            "storage",
            started,
            format!(
                "storage probe directory {} cannot be created: {error}",
                dir.display()
            ),
        );
    }
    let path = dir.join(format!(".doctor-{}", std::process::id()));
    let step = |action: &'static str| {
        let path = path.clone();
        move |error: std::io::Error| {
            format!(
                "storage probe {} cannot be {action}: {error}",
                path.display()
            )
        }
    };
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(step("created"))?;
        file.write_all(b"jeden-health-v1")
            .map_err(step("written"))?;
        file.sync_all().map_err(step("synced"))?;
        let bytes = fs::read(&path).map_err(step("read back"))?;
        if bytes != b"jeden-health-v1" {
            return Err("storage read-after-write mismatch".into());
        }
        Ok(())
    })();
    // The probe's own file is removed whatever the write found, and a removal
    // that fails is the probe's finding: the evidence names the remove step.
    let result = match (result, fs::remove_file(&path)) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(format!(
            "storage probe {} was written and read back but cannot be removed: {error}",
            path.display()
        )),
    };
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
/// then dies with "enforced sandbox unavailable" - without this probe the only
/// way to see that is to run one, and a service asking page questions through
/// such a Jeden gets empty answers for every one of them.
pub(super) fn sandbox_probe() -> HealthProbe {
    let started = Instant::now();
    let health = crate::task_runtime::sandbox::health();
    let evidence = json!({
        "backend": health.backend,
        "enforced": health.enforced,
        "helper": health.helper_path(),
        "executable": match std::env::current_exe() {
            Ok(path) => json!(path),
            Err(error) => json!({
                "error": format!("the running executable cannot be resolved: {error}")
            }),
        },
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
