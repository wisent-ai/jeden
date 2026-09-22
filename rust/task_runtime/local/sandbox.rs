use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
// Only the macOS helpers below spawn anything with piped output; `command()`
// hands its `Command` back to the caller to run.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Stdio;

#[derive(Clone, Debug)]
pub(crate) struct TaskSandboxHealth {
    pub(crate) enforced: bool,
    pub(crate) backend: &'static str,
    pub(crate) detail: String,
    helper: Option<PathBuf>,
}

impl TaskSandboxHealth {
    /// The helper this verdict was reached with, when one was found at all.
    pub(crate) fn helper_path(&self) -> Option<&Path> {
        self.helper.as_deref()
    }
}

// The three helpers below exist for the one platform whose sandbox this module
// knows how to enforce; `health()` calls them from its `target_os = "macos"` arm
// only. Without the attribute they are dead code on Linux and Windows, where the
// gate compiles with `-D warnings` and refused the build.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn helper_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = env::var_os("JEDEN_TASK_SANDBOX_HELPER") {
        candidates.push(PathBuf::from(configured));
    }
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("jeden-sandbox-helper"));
            candidates.push(directory.join("../jeden-sandbox-helper"));
            candidates.push(directory.join("../libexec/jeden-sandbox-helper"));
        }
    }
    candidates
}

/// Health checks fail closed: the verifier's own verdict is the answer, and
/// nothing here decides on its behalf that a slow machine is an unsigned
/// binary.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn health_output(command: &mut Command) -> Result<std::process::Output, String> {
    use std::io::Read;

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            stderr.read_to_end(&mut bytes)?;
        }
        Ok::<_, std::io::Error>(bytes)
    });
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for health check: {error}"))?;
    let stderr = reader
        .join()
        .map_err(|_| "health check stderr reader failed")?
        .map_err(|error| error.to_string())?;
    Ok(std::process::Output {
        status,
        stdout: Vec::new(),
        stderr,
    })
}

/// macOS only: Linux has no notion of a signed executable here, and the
/// Landlock confinement the helper applies needs no privilege to be trusted
/// with — the kernel enforces it on the process that asks, and the probe
/// below is what proves it did.
#[cfg(target_os = "macos")]
fn signed(path: &Path) -> Result<(), String> {
    let output = health_output(
        Command::new("/usr/bin/codesign")
            .arg("--verify")
            .arg("--strict")
            .args(["-R", "=anchor apple generic"])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| {
        format!(
            "cannot run codesign verification for {}: {error}",
            path.display()
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            "codesign verification failed".into()
        } else {
            detail
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn enforcement_probe(path: &Path) -> Result<(), String> {
    let output = health_output(
        Command::new(path)
            .arg("--probe")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .map_err(|error| {
        format!(
            "cannot launch sandbox enforcement probe for {}: {error}",
            path.display()
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            format!("sandbox enforcement probe exited with {}", output.status)
        } else {
            detail
        })
    }
}

pub(crate) fn health() -> TaskSandboxHealth {
    // Exactly one of these blocks survives `cfg`, so each is this function's
    // tail expression on the platform that keeps it — which is why none says
    // `return` except where it refuses early.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        TaskSandboxHealth {
            enforced: false,
            backend: "task-platform-sandbox",
            detail: "a task sandbox helper is implemented for macOS and Linux only".into(),
            helper: None,
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let backend = if cfg!(target_os = "macos") {
            "macos-seatbelt-helper"
        } else {
            "linux-landlock-helper"
        };
        let Some(helper) = helper_candidates().into_iter().find(|path| path.is_file()) else {
            return TaskSandboxHealth {
                enforced: false,
                backend,
                detail: "jeden-sandbox-helper is not installed beside the Jeden executable; install it from the same release archive or set JEDEN_TASK_SANDBOX_HELPER".into(),
                helper: None,
            };
        };
        #[cfg(target_os = "macos")]
        if let Err(error) = signed(&helper) {
            return TaskSandboxHealth {
                enforced: false,
                backend,
                detail: format!("helper signature verification failed: {error}"),
                helper: Some(helper),
            };
        }
        if let Err(error) = enforcement_probe(&helper) {
            return TaskSandboxHealth {
                enforced: false,
                backend,
                detail: format!("helper did not enforce its probe profile: {error}"),
                helper: Some(helper),
            };
        }
        TaskSandboxHealth {
            enforced: true,
            backend,
            detail: format!(
                "the helper enforced a deny-write probe ({})",
                helper.display()
            ),
            helper: Some(helper),
        }
    }
}

fn add_existing(roots: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if path.exists() {
        roots.push(path.canonicalize().unwrap_or(path));
    }
}

fn task_read_roots(program: &Path, requested: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for path in [
        "/Applications",
        "/System",
        "/Library",
        "/bin",
        "/sbin",
        "/usr",
        "/private/etc",
        "/private/var/db",
        "/private/var/folders",
        "/dev",
        // Linux carries its system files elsewhere; `add_existing` drops
        // whichever of these the running platform does not have.
        "/etc",
        "/lib",
        "/lib64",
        "/proc",
        "/run",
        "/tmp",
        "/var",
    ] {
        add_existing(&mut roots, path);
    }
    if let Some(parent) = program.parent() {
        add_existing(&mut roots, parent);
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for relative in [".jeden", ".cargo", ".rustup", ".nvm", ".local", ".config"] {
            add_existing(&mut roots, home.join(relative));
        }
    }
    for root in requested {
        add_existing(&mut roots, root);
    }
    roots.sort();
    roots.dedup();
    roots
}

pub(crate) fn command(
    program: &Path,
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
) -> Result<Command, String> {
    let health = health();
    if !health.enforced {
        return Err(format!("{}: {}", health.backend, health.detail));
    }
    let helper = health
        .helper
        .ok_or_else(|| "sandbox health was enforced without a helper path".to_string())?;
    let mut command = Command::new(helper);
    for root in task_read_roots(program, read_roots) {
        command.arg("--read").arg(root);
    }
    let mut canonical_write_roots = Vec::new();
    for root in write_roots {
        add_existing(&mut canonical_write_roots, root);
    }
    canonical_write_roots.sort();
    canonical_write_roots.dedup();
    for root in canonical_write_roots {
        command.arg("--write").arg(root);
    }
    command.arg("--").arg(program);
    Ok(command)
}
