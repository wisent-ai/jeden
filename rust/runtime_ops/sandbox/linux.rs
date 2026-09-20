use super::{SandboxHealth, SandboxState};
use crate::tool_runtime::runtime_ops::security::ExecutionGrant;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Linux confinement is the same shape as the macOS one: a helper beside the
/// executable applies the platform's own sandbox and execs the program.
/// Until 2026-09-20 this file only guessed from `/sys/kernel/security` and
/// always answered `Degraded`, so every session on a Linux fleet host
/// refused with `sandbox launcher not active` even though that kernel
/// carried Landlock.
pub(super) fn health() -> SandboxHealth {
    let health = crate::task_runtime::sandbox::health();
    SandboxHealth {
        state: if health.enforced {
            SandboxState::Enforced
        } else {
            SandboxState::Degraded
        },
        backend: health.backend,
        detail: health.detail,
    }
}

pub(super) fn command(program: &OsStr, grant: &ExecutionGrant) -> Result<Command, String> {
    crate::task_runtime::sandbox::command(
        Path::new(program),
        &grant.filesystem.read_roots,
        &grant.filesystem.write_roots,
    )
}
