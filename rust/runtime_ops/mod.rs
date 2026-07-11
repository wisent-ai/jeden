pub mod kernel;
pub mod pty;
mod output;
mod process;

pub use output::{ArtifactSink, BoundedOutput, OutputCapture, OutputLimits};
pub use process::{ManagedCommand, ManagedProcessResult, ProcessManager, TerminationReason};

#[derive(Clone, Debug)]
pub struct SessionRuntimeDescriptor {
    pub name: &'static str,
    pub healthy: bool,
    pub backend: &'static str,
    pub detail: Option<String>,
}

pub fn session_runtime_descriptors(cwd: &std::path::Path) -> Vec<SessionRuntimeDescriptor> {
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
            detail: if eval_detail.is_empty() { None } else { Some(eval_detail) },
        },
        SessionRuntimeDescriptor {
            name: "pty_session",
            healthy: pty.is_ok(),
            backend: "openpty-sh",
            detail: pty.err(),
        },
    ]
}

pub(crate) fn capability_descriptors(cwd: &std::path::Path) -> Vec<crate::capability::CapabilityDescriptor> {
    use crate::capability::{CapabilityDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget};
    use serde_json::json;

    let mut descriptors = Vec::new();
    for runtime in session_runtime_descriptors(cwd) {
        let health = if runtime.healthy {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(runtime.detail.clone().unwrap_or_else(|| "runtime probe failed".into()))
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
            .policy(CapabilityPolicy::ApprovalRequired)
            .health(health.clone())
            .metadata(json!({"backend":runtime.backend,"input":input}));
            if runtime.healthy { descriptor = descriptor.executable(name); }
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
    cancellation: CancellationToken,
    deadline: Option<Instant>,
    progress: ProgressSink<'a>,
    artifacts: ArtifactSink,
    output_limits: OutputLimits,
}

impl std::fmt::Debug for OperationContext<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationContext")
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("deadline", &self.deadline)
            .field("artifacts", &self.artifacts)
            .field("output_limits", &self.output_limits)
            .finish_non_exhaustive()
    }
}

impl<'a> OperationContext<'a> {
    pub fn new(cancellation: CancellationToken, artifacts: ArtifactSink) -> Self {
        Self {
            cancellation,
            deadline: None,
            progress: Arc::new(|_| {}),
            artifacts,
            output_limits: OutputLimits::default(),
        }
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
        (self.progress)(event);
    }

    pub fn artifacts(&self) -> &ArtifactSink {
        &self.artifacts
    }

    pub fn output_limits(&self) -> OutputLimits {
        self.output_limits
    }
}
