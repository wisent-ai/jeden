//! What one operation carries with it while it runs: who asked, what it may
//! do, where its output goes, and the flag that says it was cancelled.
//!
//! Split out of `runtime_ops/mod.rs`, which had grown past the module line cap.

use super::{ArtifactSink, ExecutionGrant, GrantError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use crate::tool_runtime::runtime_ops::TraceContext;
use crate::tool_runtime::runtime_ops::output::OutputLimits;
use crate::tool_runtime::runtime_ops::security::TelemetryPolicy;

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
