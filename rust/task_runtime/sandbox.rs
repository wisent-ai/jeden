use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
// Only the macOS helpers below spawn anything with piped output; `command()`
// hands its `Command` back to the caller to run.
#[cfg(target_os = "macos")]
use std::process::Stdio;

#[derive(Clone, Debug)]
pub(crate) struct TaskSandboxHealth {
    pub(crate) enforced: bool,
    pub(crate) backend: &'static str,
    pub(crate) detail: String,
    helper: Option<PathBuf>,
}

// The three helpers below exist for the one platform whose sandbox this module
// knows how to enforce; `health()` calls them from its `target_os = "macos"` arm
// only. Without the attribute they are dead code on Linux and Windows, where the
// gate compiles with `-D warnings` and refused the build.
#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn signed(path: &Path) -> Result<(), String> {
    let output = Command::new("/usr/bin/codesign")
        .arg("--verify")
        .arg("--strict")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("cannot run codesign verification: {error}"))?;
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

#[cfg(target_os = "macos")]
fn enforcement_probe(path: &Path) -> Result<(), String> {
    let output = Command::new(path)
        .arg("--probe")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("cannot launch sandbox enforcement probe: {error}"))?;
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
    // Exactly one of these two blocks survives `cfg`, so each is this function's
    // tail expression on the platform that keeps it — which is why neither says
    // `return`.
    #[cfg(not(target_os = "macos"))]
    {
        TaskSandboxHealth {
            enforced: false,
            backend: "task-platform-sandbox",
            detail: "a signed task sandbox helper is currently implemented only for macOS".into(),
            helper: None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let Some(helper) = helper_candidates().into_iter().find(|path| path.is_file()) else {
            return TaskSandboxHealth {
                enforced: false,
                backend: "macos-seatbelt-helper",
                detail: "jeden-sandbox-helper is not installed beside the Jeden executable; build and code-sign it or set JEDEN_TASK_SANDBOX_HELPER".into(),
                helper: None,
            };
        };
        if let Err(error) = signed(&helper) {
            return TaskSandboxHealth {
                enforced: false,
                backend: "macos-seatbelt-helper",
                detail: format!("helper signature is invalid: {error}"),
                helper: Some(helper),
            };
        }
        if let Err(error) = enforcement_probe(&helper) {
            return TaskSandboxHealth {
                enforced: false,
                backend: "macos-seatbelt-helper",
                detail: format!("helper did not enforce its probe profile: {error}"),
                helper: Some(helper),
            };
        }
        TaskSandboxHealth {
            enforced: true,
            backend: "macos-seatbelt-helper",
            detail: format!(
                "signed helper enforced a deny-write Seatbelt probe ({})",
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
