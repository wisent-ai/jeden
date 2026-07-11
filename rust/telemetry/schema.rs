use serde::{Deserialize, Serialize};

/// Opaque, pseudonymized correlation identifier. Raw identifiers are never serialized.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrivateId(String);

impl PrivateId {
    pub(crate) fn from_digest(value: String) -> Self {
        debug_assert!(value.starts_with("pid_"));
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrelationIds {
    pub operation_id: PrivateId,
    pub session_id: PrivateId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_operation_id: Option<PrivateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<PrivateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_id: Option<PrivateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<PrivateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_generation: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    Operation,
    ModelAttempt,
    RouteSelection,
    ToolExecution,
    WorkerAttempt,
    Cancellation,
    Retry,
    Failover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TracePhase {
    Started,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Succeeded,
    Cancelled,
    DeadlineExceeded,
    Denied,
    Failed,
    ExternalBlocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceEvent {
    pub kind: TraceKind,
    pub phase: TracePhase,
    pub ids: CorrelationIds,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<TerminalState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_micros: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricName {
    OperationStarted,
    OperationCompleted,
    OperationFailed,
    OperationCancelled,
    Retry,
    Failover,
    TelemetryDropped,
    TelemetryWriteFailed,
    TelemetryExportFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetricEvent {
    pub name: MetricName,
    pub value: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<CorrelationIds>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    GrantIssued,
    GrantDenied,
    ApprovalGranted,
    ApprovalDenied,
    SecretAccessGranted,
    SecretAccessDenied,
    SandboxViolation,
    NetworkDenied,
    FilesystemDenied,
    ProcessDenied,
    TelemetryDeleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    Allowed,
    Denied,
    Observed,
}

/// Security audit entries deliberately contain no message, labels, resource names, or payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditEvent {
    pub action: AuditAction,
    pub decision: AuditDecision,
    pub subject_id: PrivateId,
    pub ids: CorrelationIds,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", content = "data", rename_all = "snake_case")]
pub enum TelemetryRecord {
    Trace(TraceEvent),
    Metric(MetricEvent),
    Audit(AuditEvent),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetryEnvelope {
    pub schema_version: u16,
    pub sequence: u64,
    pub observed_at_unix_ms: u64,
    pub record: TelemetryRecord,
}

impl TelemetryEnvelope {
    pub const SCHEMA_VERSION: u16 = 1;
}

/// Canonical serialized field allowlist used by schema/conformance checks.
pub const ALLOWLISTED_FIELDS: &[&str] = &[
    "schemaVersion",
    "sequence",
    "observedAtUnixMs",
    "record",
    "signal",
    "data",
    "kind",
    "phase",
    "ids",
    "terminalState",
    "durationMicros",
    "name",
    "value",
    "action",
    "decision",
    "subjectId",
    "operationId",
    "sessionId",
    "parentOperationId",
    "attemptId",
    "routeId",
    "capabilityId",
    "capabilityGeneration",
];
