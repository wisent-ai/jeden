use super::{SandboxHealth, SandboxState};
pub(super) fn health() -> SandboxHealth {
    SandboxHealth { state: SandboxState::Degraded, backend: "macos-seatbelt-helper", detail: "signed sandbox helper and entitlements are not installed; deprecated sandbox-exec is not accepted as enforcement".into() }
}
