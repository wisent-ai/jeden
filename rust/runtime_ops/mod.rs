mod host;
pub mod kernel;
mod output;
#[path = "../platform/mod.rs"]
pub mod platform;
pub mod sandbox;
pub mod secrets;
pub mod security;
mod context;

pub use context::{
    CancellationToken, OperationContext, OperationProgress, ProgressSink,
};


pub use host::{fs, network, pty};
use host::process;

pub use output::{ArtifactSink, BoundedOutput, OutputCapture, OutputLimits};
pub use process::{ManagedCommand, ManagedProcessResult, ProcessManager, TerminationReason};
pub use security::{
    ExecutionGrant, FsGrant, GrantError, NetworkGrant, Principal, PrincipalKind, ProcessGrant,
    ResourceLimits, SandboxRequirement, SecretGrant, TelemetryPolicy,
};

#[derive(Clone, Debug)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
}

#[derive(Clone, Debug)]
pub struct SecureRuntime {
    health: sandbox::SandboxHealth,
}
impl SecureRuntime {
    pub fn detect() -> Self {
        Self {
            health: sandbox::platform_health(),
        }
    }
    pub fn health(&self) -> &sandbox::SandboxHealth {
        &self.health
    }
    pub fn authorize(&self, grant: &ExecutionGrant) -> Result<(), GrantError> {
        if grant.is_expired() {
            return Err(GrantError::Expired);
        }
        sandbox::require_enforced(grant).map(|_| ())
    }
}

/// Derive the effective authority for an untrusted child. Untrusted runtimes always
/// require an enforced platform sandbox; a caller cannot weaken that requirement.
pub fn untrusted_child<'a>(
    context: &OperationContext<'a>,
    operation_id: impl Into<String>,
) -> Result<OperationContext<'a>, GrantError> {
    let mut requested = context.execution_grant().clone();
    requested.sandbox = SandboxRequirement::Enforced;
    let child = context.child(operation_id, &requested)?;
    SecureRuntime::detect().authorize(child.execution_grant())?;
    Ok(child)
}

#[derive(Clone, Debug)]
pub struct SessionRuntimeDescriptor {
    pub name: &'static str,
    pub healthy: bool,
    pub backend: &'static str,
    pub detail: Option<String>,
}

pub fn session_runtime_descriptors(cwd: &std::path::Path) -> Vec<SessionRuntimeDescriptor> {
    let sandbox = sandbox::platform_health();
    if !sandbox.enforced() {
        return vec![
            SessionRuntimeDescriptor {
                name: "eval_session",
                healthy: false,
                backend: "persistent-python-javascript",
                detail: Some(format!(
                    "sandbox {} is not enforced: {}",
                    sandbox.backend, sandbox.detail
                )),
            },
            SessionRuntimeDescriptor {
                name: "pty_session",
                healthy: false,
                backend: "openpty-sh",
                detail: Some(format!(
                    "sandbox {} is not enforced: {}",
                    sandbox.backend, sandbox.detail
                )),
            },
        ];
    }
    let python = kernel::probe(kernel::KernelLanguage::Python, cwd);
    let javascript = kernel::probe(kernel::KernelLanguage::JavaScript, cwd);
    let eval_detail = [python.as_ref().err(), javascript.as_ref().err()]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let pty = pty::probe(cwd);
    vec![
        SessionRuntimeDescriptor {
            name: "eval_session",
            healthy: python.is_ok() && javascript.is_ok(),
            backend: "persistent-python-javascript",
            detail: if eval_detail.is_empty() {
                None
            } else {
                Some(eval_detail)
            },
        },
        SessionRuntimeDescriptor {
            name: "pty_session",
            healthy: pty.is_ok(),
            backend: "openpty-sh",
            detail: pty.err(),
        },
    ]
}

pub(crate) fn capability_descriptors(
    cwd: &std::path::Path,
) -> Vec<crate::capability::CapabilityDescriptor> {
    use crate::capability::{
        CapabilityDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget,
    };
    use serde_json::json;

    let mut descriptors = Vec::new();
    for runtime in session_runtime_descriptors(cwd) {
        let health = if runtime.healthy {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(
                runtime
                    .detail
                    .clone()
                    .unwrap_or_else(|| "runtime probe failed".into()),
            )
        };
        let tools = match runtime.name {
            "eval_session" => vec![(
                "eval_session",
                "Evaluate code in a persistent bounded Python or JavaScript kernel",
                json!({"type":"object","properties":{"language":{"type":"string","enum":["python","javascript"]},"code":{"type":"string"},"reset":{"type":"boolean"}},"required":["language","code"]}),
            )],
            "pty_session" => vec![
                (
                    "pty_session",
                    "Send input to a persistent bounded pseudo-terminal shell",
                    json!({"type":"object","properties":{"input":{"type":"string"},"reset":{"type":"boolean"}},"required":["input"]}),
                ),
                (
                    "pty_resize",
                    "Resize a live persistent pseudo-terminal session without spawning a new process",
                    json!({"type":"object","properties":{"sessionId":{"type":"string"},"cols":{"type":"integer","minimum":pty::MIN_PTY_COLS,"maximum":pty::MAX_PTY_COLS},"rows":{"type":"integer","minimum":pty::MIN_PTY_ROWS,"maximum":pty::MAX_PTY_ROWS}},"required":["sessionId","cols","rows"]}),
                ),
            ],
            _ => unreachable!("session runtime descriptors are stable"),
        };
        for (name, description, input) in tools {
            let mut descriptor = CapabilityDescriptor::new(
                format!("tool/{name}"),
                CapabilityKind::Tool,
                "runtime-ops",
                name,
                description,
                FunctionTarget::BuiltinTool { name: name.into() },
            )
            .operation(name)
            .policy(CapabilityPolicy::Sandboxed)
            .health(health.clone())
            .metadata(json!({"backend":runtime.backend,"input":input}));
            if runtime.healthy {
                descriptor = descriptor.executable(name);
            }
            descriptors.push(descriptor);
        }
    }
    descriptors
}
