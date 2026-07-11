use super::{SandboxHealth, SandboxState};
pub(super) fn health() -> SandboxHealth {
    SandboxHealth { state: SandboxState::Degraded, backend:"windows-job-restricted-token", detail:"Job Object/restricted-token launcher is not active; process creation cannot be claimed sandboxed".into() }
}
