#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use crate::tool_runtime::runtime_ops::security::{ExecutionGrant, GrantError, SandboxRequirement};
use std::ffi::OsStr;
use std::process::Command;

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

pub fn command(program: &OsStr, grant: &ExecutionGrant) -> Result<Command, GrantError> {
    if grant.sandbox == SandboxRequirement::Enforced {
        require_enforced(grant)?;
        #[cfg(target_os = "macos")]
        {
            return macos::command(program, grant).map_err(GrantError::SandboxUnavailable);
        }
    }
    Ok(Command::new(program))
}
