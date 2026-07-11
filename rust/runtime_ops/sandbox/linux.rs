use super::{SandboxHealth, SandboxState};
pub(super) fn health() -> SandboxHealth {
    let landlock = std::path::Path::new("/sys/kernel/security/landlock").exists();
    let cgroup = std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
    SandboxHealth { state: SandboxState::Degraded, backend:"linux-landlock-seccomp-cgroup", detail:format!("sandbox launcher not active (landlock={landlock}, cgroup_v2={cgroup}); detection alone is not enforcement") }
}
