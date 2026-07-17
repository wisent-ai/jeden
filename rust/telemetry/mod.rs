mod export;
mod privacy;
mod recorder;
mod schema;

pub use export::{ExportError, ExportStatus, OtlpExporter};
pub use privacy::{contains_canary, PrivacyFilter};
pub use recorder::{
    CaptureStatus, RetentionReport, TelemetryConfig, TelemetryHealth, TelemetryRecorder,
};
pub use schema::{
    AuditAction, AuditDecision, AuditEvent, CorrelationIds, MetricEvent, MetricName, PrivateId,
    TelemetryEnvelope, TelemetryRecord, TerminalState, TraceEvent, TraceKind, TracePhase,
    ALLOWLISTED_FIELDS,
};

use std::sync::Arc;

/// Adapter implemented by OperationContextV2 after its security-owned shape is finalized.
/// Keeping this trait here avoids a competing operation context or authority-bearing clone.
pub trait TelemetryContextAdapter {
    fn telemetry(&self) -> Option<&TelemetryHandle>;
}

/// Narrow operation-scoped facade: callers can emit only closed-schema events, never arbitrary
/// labels, messages, prompts, paths, URLs, hosts, command arguments, or tool I/O.
#[derive(Clone)]
pub struct TelemetryHandle {
    recorder: Arc<TelemetryRecorder>,
    privacy: PrivacyFilter,
    ids: CorrelationIds,
}

impl std::fmt::Debug for TelemetryHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelemetryHandle")
            .field("ids", &self.ids)
            .finish_non_exhaustive()
    }
}

impl TelemetryHandle {
    pub fn new(
        recorder: Arc<TelemetryRecorder>,
        privacy: PrivacyFilter,
        ids: CorrelationIds,
    ) -> Self {
        Self {
            recorder,
            privacy,
            ids,
        }
    }

    pub fn ids(&self) -> &CorrelationIds {
        &self.ids
    }

    pub fn child(&self, raw_operation_id: &str) -> Self {
        let mut ids = self.ids.clone();
        ids.parent_operation_id = Some(self.ids.operation_id.clone());
        ids.operation_id = self.privacy.pseudonymize("operation", raw_operation_id);
        ids.attempt_id = None;
        Self {
            recorder: self.recorder.clone(),
            privacy: self.privacy.clone(),
            ids,
        }
    }

    pub fn trace(
        &self,
        kind: TraceKind,
        phase: TracePhase,
        terminal_state: Option<TerminalState>,
        duration_micros: Option<u64>,
    ) -> CaptureStatus {
        self.recorder.capture(TelemetryRecord::Trace(TraceEvent {
            kind,
            phase,
            ids: self.ids.clone(),
            terminal_state,
            duration_micros,
        }))
    }

    pub fn metric(&self, name: MetricName, value: u64) -> CaptureStatus {
        self.recorder.capture(TelemetryRecord::Metric(MetricEvent {
            name,
            value,
            ids: Some(self.ids.clone()),
        }))
    }

    pub fn audit(
        &self,
        action: AuditAction,
        decision: AuditDecision,
        raw_subject_id: &str,
    ) -> CaptureStatus {
        self.recorder.capture(TelemetryRecord::Audit(AuditEvent {
            action,
            decision,
            subject_id: self.privacy.pseudonymize("audit-subject", raw_subject_id),
            ids: self.ids.clone(),
        }))
    }
}
