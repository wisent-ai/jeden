#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod test_backend {
    use super::{SandboxHealth, SandboxState};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    #[cfg(target_os = "macos")]
    use std::process::Stdio;

    thread_local! {
        static ACTIVE: Cell<bool> = const { Cell::new(false) };
    }

    pub(crate) struct Guard {
        previous: bool,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.set(self.previous);
        }
    }

    pub(crate) fn install(node: &str) -> Result<Guard, String> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = node;
            return Err("the injected Seatbelt test backend requires macOS".into());
        }
        #[cfg(target_os = "macos")]
        {
            let probe = Command::new("/usr/bin/sandbox-exec")
                .args(["-p", base_profile(), node, "--permission", "-e", ""])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("test sandbox probe failed to start: {error}"))?;
            if !probe.status.success() {
                return Err(format!(
                    "test sandbox probe failed: {}",
                    String::from_utf8_lossy(&probe.stderr).trim()
                ));
            }
            let previous = ACTIVE.replace(true);
            Ok(Guard { previous })
        }
    }

    pub(super) fn health() -> Option<SandboxHealth> {
        ACTIVE.get().then(|| SandboxHealth {
            state: SandboxState::Enforced,
            backend: "test-macos-seatbelt-node-permissions",
            detail: "injected test backend composes Seatbelt network/process isolation with Node filesystem permissions".into(),
        })
    }

    fn base_profile() -> &'static str {
        "(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n(allow sysctl-read)\n(allow mach-lookup)\n(allow signal)\n(allow ipc-posix*)\n"
    }

    fn canonical(path: &Path) -> PathBuf {
        if let Ok(canonical) = path.canonicalize() {
            return canonical;
        }
        if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
            if let Ok(parent) = parent.canonicalize() {
                return parent.join(name);
            }
        }
        path.to_path_buf()
    }

    fn profile_path(path: &Path) -> String {
        canonical(path)
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    pub(crate) fn command(
        node: &str,
        read_paths: &[PathBuf],
        write_path: Option<&Path>,
        allow_command: bool,
    ) -> Result<Option<Command>, String> {
        if !ACTIVE.get() {
            return Ok(None);
        }
        let mut profile = base_profile().to_string();
        if let Some(path) = write_path {
            std::fs::create_dir_all(path)
                .map_err(|error| format!("test sandbox write root unavailable: {error}"))?;
        }
        if let Some(path) = write_path {
            profile.push_str(&format!(
                "(allow file-write* (subpath \"{}\"))\n",
                profile_path(path)
            ));
        }
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command.args(["-p", &profile, node, "--permission"]);
        for path in read_paths {
            command.arg(format!("--allow-fs-read={}", path.display()));
        }
        if let Some(path) = write_path {
            command.arg(format!("--allow-fs-write={}", path.display()));
            command.arg(format!("--allow-fs-write={}", canonical(path).display()));
        }
        if allow_command {
            command.arg("--allow-child-process");
        }
        Ok(Some(command))
    }
}

#[cfg(test)]
pub(crate) use test_backend::Guard as TestSandboxGuard;

#[cfg(test)]
pub(crate) fn install_test_backend(node: &str) -> Result<TestSandboxGuard, String> {
    test_backend::install(node)
}

#[cfg(test)]
pub(crate) fn test_command(
    node: &str,
    read_paths: &[std::path::PathBuf],
    write_path: Option<&std::path::Path>,
    allow_command: bool,
) -> Result<Option<std::process::Command>, String> {
    test_backend::command(node, read_paths, write_path, allow_command)
}

use crate::tool_runtime::runtime_ops::security::{ExecutionGrant, GrantError, SandboxRequirement};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxState {
    Enforced,
    Degraded,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxHealth {
    pub state: SandboxState,
    pub backend: &'static str,
    pub detail: String,
}
impl SandboxHealth {
    pub fn enforced(&self) -> bool {
        self.state == SandboxState::Enforced
    }
}

pub fn platform_health() -> SandboxHealth {
    #[cfg(test)]
    if let Some(health) = test_backend::health() {
        return health;
    }
    #[cfg(target_os = "macos")]
    {
        return macos::health();
    }
    #[cfg(target_os = "linux")]
    {
        return linux::health();
    }
    #[cfg(target_os = "windows")]
    {
        return windows::health();
    }
    #[allow(unreachable_code)]
    SandboxHealth {
        state: SandboxState::Unsupported,
        backend: "none",
        detail: "platform has no sandbox backend".into(),
    }
}

pub fn require_enforced(grant: &ExecutionGrant) -> Result<SandboxHealth, GrantError> {
    let health = platform_health();
    if grant.sandbox == SandboxRequirement::Enforced && !health.enforced() {
        return Err(GrantError::SandboxUnavailable(format!(
            "{}: {}",
            health.backend, health.detail
        )));
    }
    Ok(health)
}
