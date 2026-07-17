use super::{SandboxHealth, SandboxState};
use crate::tool_runtime::runtime_ops::security::ExecutionGrant;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

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
