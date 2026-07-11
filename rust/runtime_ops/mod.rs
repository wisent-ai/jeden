pub mod fs;
pub mod kernel;
pub mod network;
mod output;
#[path = "../platform/mod.rs"]
pub mod platform;
mod process;
pub mod pty;
pub mod sandbox;
pub mod secrets;
pub mod security;

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
                json!({"type":"object","properties":{"language":{"type":"string","enum":["python","javascript"]},"code":{"type":"string"},"reset":{"type":"boolean"},"timeoutMs":{"type":"number"}},"required":["language","code"]}),
            )],
            "pty_session" => vec![
                (
                    "pty_session",
                    "Send input to a persistent bounded pseudo-terminal shell",
                    json!({"type":"object","properties":{"input":{"type":"string"},"reset":{"type":"boolean"},"timeoutMs":{"type":"number"}},"required":["input"]}),
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

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct OperationProgress {
    pub stream: &'static str,
    pub bytes: u64,
    pub total_bytes: u64,
}

pub type ProgressSink<'a> = Arc<dyn Fn(OperationProgress) + 'a>;

#[derive(Clone)]
pub struct OperationContext<'a> {
    operation_id: String,
    session_id: Option<String>,
    turn_id: Option<String>,
    parent_operation_id: Option<String>,
    cancellation: CancellationToken,
    deadline: Option<Instant>,
    progress: ProgressSink<'a>,
    artifacts: ArtifactSink,
    output_limits: OutputLimits,
    approval_handle: Option<String>,
    ledger_handle: Option<String>,
    trace_context: Option<TraceContext>,
    execution_grant: ExecutionGrant,
    telemetry_policy: TelemetryPolicy,
    telemetry: Option<crate::telemetry::TelemetryHandle>,
}

impl std::fmt::Debug for OperationContext<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationContext")
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("deadline", &self.deadline)
            .field("artifacts", &self.artifacts)
            .field("output_limits", &self.output_limits)
            .field("operation_id", &self.operation_id)
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("parent_operation_id", &self.parent_operation_id)
            .field("principal", &self.execution_grant.principal)
            .finish_non_exhaustive()
    }
}

impl<'a> OperationContext<'a> {
    pub fn new(cancellation: CancellationToken, artifacts: ArtifactSink) -> Self {
        static NEXT_OPERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let operation_id = format!(
            "op-{}-{}",
            std::process::id(),
            NEXT_OPERATION.fetch_add(1, Ordering::Relaxed)
        );
        let root = std::env::current_dir().unwrap_or_else(|_| artifacts.root().to_path_buf());
        Self {
            operation_id,
            session_id: None,
            turn_id: None,
            parent_operation_id: None,
            cancellation,
            deadline: None,
            progress: Arc::new(|_| {}),
            artifacts,
            output_limits: OutputLimits::default(),
            approval_handle: None,
            ledger_handle: None,
            trace_context: None,
            execution_grant: ExecutionGrant::trusted_host("jeden-host", root),
            telemetry_policy: TelemetryPolicy::Disabled,
            telemetry: None,
        }
    }
    pub fn with_identity(
        mut self,
        operation_id: impl Into<String>,
        session_id: Option<String>,
        turn_id: Option<String>,
        parent_operation_id: Option<String>,
    ) -> Self {
        self.operation_id = operation_id.into();
        self.session_id = session_id;
        self.turn_id = turn_id;
        self.parent_operation_id = parent_operation_id;
        self
    }
    pub fn with_execution_grant(mut self, grant: ExecutionGrant) -> Self {
        self.artifacts = self.artifacts.with_grant(grant.clone());
        self.execution_grant = grant;
        self
    }
    pub fn with_handles(mut self, approval: Option<String>, ledger: Option<String>) -> Self {
        self.approval_handle = approval;
        self.ledger_handle = ledger;
        self
    }
    pub fn with_trace(mut self, trace: TraceContext) -> Self {
        self.trace_context = Some(trace);
        self
    }
    pub fn with_telemetry_policy(mut self, policy: TelemetryPolicy) -> Self {
        self.telemetry_policy = policy;
        if policy == TelemetryPolicy::Disabled {
            self.telemetry = None;
        }
        self
    }
    pub fn with_telemetry(
        mut self,
        policy: TelemetryPolicy,
        telemetry: crate::telemetry::TelemetryHandle,
    ) -> Self {
        self.telemetry_policy = policy;
        self.telemetry = if policy == TelemetryPolicy::Disabled {
            None
        } else {
            Some(telemetry)
        };
        self
    }
    pub fn child(
        &self,
        operation_id: impl Into<String>,
        requested: &ExecutionGrant,
    ) -> Result<OperationContext<'a>, GrantError> {
        let grant = self.execution_grant.intersect(requested)?;
        let operation_id = operation_id.into();
        let telemetry = self
            .telemetry
            .as_ref()
            .map(|handle| handle.child(&operation_id));
        Ok(Self {
            operation_id,
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            parent_operation_id: Some(self.operation_id.clone()),
            cancellation: self.cancellation.clone(),
            deadline: self.deadline,
            progress: self.progress.clone(),
            artifacts: self.artifacts.clone().with_grant(grant.clone()),
            output_limits: self.output_limits,
            approval_handle: self.approval_handle.clone(),
            ledger_handle: self.ledger_handle.clone(),
            trace_context: self.trace_context.clone(),
            execution_grant: grant,
            telemetry_policy: self.telemetry_policy,
            telemetry,
        })
    }
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }
    pub fn with_progress(mut self, progress: ProgressSink<'a>) -> Self {
        self.progress = progress;
        self
    }
    pub fn with_output_limits(mut self, limits: OutputLimits) -> Self {
        self.output_limits = limits;
        self
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }
    pub fn parent_operation_id(&self) -> Option<&str> {
        self.parent_operation_id.as_deref()
    }
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn effective_deadline(&self, timeout: Duration) -> Instant {
        let local = Instant::now() + timeout;
        self.deadline.map_or(local, |parent| parent.min(local))
    }
    pub fn progress(&self, event: OperationProgress) {
        (self.progress)(event)
    }
    pub fn artifacts(&self) -> &ArtifactSink {
        &self.artifacts
    }
    pub fn output_limits(&self) -> OutputLimits {
        self.output_limits
    }
    pub fn approval_handle(&self) -> Option<&str> {
        self.approval_handle.as_deref()
    }
    pub fn ledger_handle(&self) -> Option<&str> {
        self.ledger_handle.as_deref()
    }
    pub fn trace_context(&self) -> Option<&TraceContext> {
        self.trace_context.as_ref()
    }
    pub fn execution_grant(&self) -> &ExecutionGrant {
        &self.execution_grant
    }
    pub fn telemetry_policy(&self) -> TelemetryPolicy {
        self.telemetry_policy
    }
    pub fn telemetry(&self) -> Option<&crate::telemetry::TelemetryHandle> {
        self.telemetry.as_ref()
    }
}

impl crate::telemetry::TelemetryContextAdapter for OperationContext<'_> {
    fn telemetry(&self) -> Option<&crate::telemetry::TelemetryHandle> {
        self.telemetry()
    }
}
