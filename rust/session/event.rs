use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckpointPayloadV2 {
    pub(crate) label: Option<String>,
    pub(crate) messages: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RewindPayloadV2 {
    pub(crate) checkpoint_id: String,
    pub(crate) from_leaf_id: String,
}

pub(crate) const SESSION_EVENT_SCHEMA_VERSION: u32 = 2;

/// Closed session vocabulary. A variant is added here before a producer can
/// persist it, preventing misspelled/stringly event kinds from entering replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(crate) enum SessionPayloadV2 {
    Message(Value),
    User(Value),
    Assistant(Value),
    AssistantRaw(Value),
    Final(Value),
    Action(Value),
    ToolCall(Value),
    ToolResult(Value),
    Approval(Value),
    Artifact(Value),
    ContextSnapshot(Value),
    Compaction(Value),
    AutoCompaction(Value),
    AutoCompactionError(Value),
    AutoContinue(Value),
    ToolPrune(Value),
    Handoff(Value),
    Lineage(Value),
    Branch(Value),
    Checkpoint(Value),
    Rewind(Value),
    GoalLifecycle(Value),
    MemoryMutation(Value),
    RoadmapItemCreated(Value),
    RoadmapItemUpdated(Value),
    RoadmapItemStarted(Value),
    RoadmapItemBlocked(Value),
    RoadmapEvidenceAttached(Value),
    RoadmapItemPassed(Value),
    RoadmapItemDropped(Value),
    MemoryRecall(Value),
    ModelAttempt(Value),
    ModelRoute(Value),
    ModelRouteResult(Value),
    ModelRetry(Value),
    ModelUsage(Value),
    UsageError(Value),
    CapabilityGeneration(Value),
    WorkerJob(Value),
    WorkerAttempt(Value),
    WorkerLease(Value),
    WorkerEvent(Value),
    Collaboration(Value),
    Interaction(Value),
    TelemetryReference(Value),
    TerminalOutcome(Value),
    RunError(Value),
    Advisor(Value),
    Agent(Value),
    AgentState(Value),
    PendingPreview(Value),
    PendingClaim(Value),
    PendingApply(Value),
    PendingDiscard(Value),
    PendingExpire(Value),
    /// The task contract was not met: `rule`, `outcome`, and a human `message`.
    ContractViolation(Value),
    TaskContract(Value),
    TaskReport(Value),
}

impl SessionPayloadV2 {
    pub(crate) fn from_legacy(kind: &str, data: Value) -> Result<Self, String> {
        Ok(match kind {
            "message" => Self::Message(data),
            "user" => Self::User(data),
            "assistant" => Self::Assistant(data),
            "assistant_raw" => Self::AssistantRaw(data),
            "final" => Self::Final(data),
            "action" => Self::Action(data),
            "tool_call" => Self::ToolCall(data),
            "tool_result" => Self::ToolResult(data),
            "approval" => Self::Approval(data),
            "artifact" => Self::Artifact(data),
            "context_snapshot" => Self::ContextSnapshot(data),
            "compaction" => Self::Compaction(data),
            "auto_compaction" => Self::AutoCompaction(data),
            "auto_compaction_error" => Self::AutoCompactionError(data),
            "auto_continue" => Self::AutoContinue(data),
            "tool_prune" => Self::ToolPrune(data),
            "handoff" => Self::Handoff(data),
            "lineage" => Self::Lineage(data),
            "branch" => Self::Branch(data),
            "checkpoint" => Self::Checkpoint(data),
            "rewind" => Self::Rewind(data),
            "goal_lifecycle" => Self::GoalLifecycle(data),
            "memory_mutation" => Self::MemoryMutation(data),
            "memory_recall" => Self::MemoryRecall(data),
            "roadmap_item_created" => Self::RoadmapItemCreated(data),
            "roadmap_item_updated" | "roadmap_acceptance_updated" => Self::RoadmapItemUpdated(data),
            "roadmap_item_started" => Self::RoadmapItemStarted(data),
            "roadmap_item_blocked" => Self::RoadmapItemBlocked(data),
            "roadmap_evidence_attached" => Self::RoadmapEvidenceAttached(data),
            "roadmap_item_passed" => Self::RoadmapItemPassed(data),
            "roadmap_item_dropped" => Self::RoadmapItemDropped(data),
            "model_attempt" => Self::ModelAttempt(data),
            "model_route" => Self::ModelRoute(data),
            "model_route_result" => Self::ModelRouteResult(data),
            "model_retry" => Self::ModelRetry(data),
            "model_usage" => Self::ModelUsage(data),
            "usage_error" => Self::UsageError(data),
            "capability_generation" => Self::CapabilityGeneration(data),
            "worker_job" => Self::WorkerJob(data),
            "worker_attempt" => Self::WorkerAttempt(data),
            "worker_lease" => Self::WorkerLease(data),
            "worker_event" => Self::WorkerEvent(data),
            "collaboration" => Self::Collaboration(data),
            "interaction" => Self::Interaction(data),
            "telemetry_reference" => Self::TelemetryReference(data),
            "terminal_outcome" => Self::TerminalOutcome(data),
            "run_error" => Self::RunError(data),
            "advisor" => Self::Advisor(data),
            "agent" => Self::Agent(data),
            "agent_state" => Self::AgentState(data),
            "pending_preview" => Self::PendingPreview(data),
            "pending_claim" => Self::PendingClaim(data),
            "pending_apply" => Self::PendingApply(data),
            "pending_discard" => Self::PendingDiscard(data),
            "pending_expire" => Self::PendingExpire(data),
            "contract_violation" => Self::ContractViolation(data),
            "task_contract" => Self::TaskContract(data),
            "task_report" => Self::TaskReport(data),
            _ => return Err(format!("unsupported session event type: {kind}")),
        })
    }

    pub(crate) fn checkpoint(payload: CheckpointPayloadV2) -> Result<Self, String> {
        serde_json::to_value(payload)
            .map(Self::Checkpoint)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn rewind(payload: RewindPayloadV2) -> Result<Self, String> {
        serde_json::to_value(payload)
            .map(Self::Rewind)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn checkpoint_data(&self) -> Result<CheckpointPayloadV2, String> {
        let Self::Checkpoint(value) = self else {
            return Err("session event is not a checkpoint".into());
        };
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }

    pub(crate) fn rewind_data(&self) -> Result<RewindPayloadV2, String> {
        let Self::Rewind(value) = self else {
            return Err("session event is not a rewind".into());
        };
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::AssistantRaw(_) => "assistant_raw",
            Self::Final(_) => "final",
            Self::Action(_) => "action",
            Self::ToolCall(_) => "tool_call",
            Self::ToolResult(_) => "tool_result",
            Self::Approval(_) => "approval",
            Self::Artifact(_) => "artifact",
            Self::ContextSnapshot(_) => "context_snapshot",
            Self::Compaction(_) => "compaction",
            Self::AutoCompaction(_) => "auto_compaction",
            Self::AutoCompactionError(_) => "auto_compaction_error",
            Self::AutoContinue(_) => "auto_continue",
            Self::ToolPrune(_) => "tool_prune",
            Self::Handoff(_) => "handoff",
            Self::Lineage(_) => "lineage",
            Self::Branch(_) => "branch",
            Self::Checkpoint(_) => "checkpoint",
            Self::Rewind(_) => "rewind",
            Self::GoalLifecycle(_) => "goal_lifecycle",
            Self::MemoryMutation(_) => "memory_mutation",
            Self::RoadmapItemCreated(_) => "roadmap_item_created",
            Self::RoadmapItemUpdated(_) => "roadmap_item_updated",
            Self::RoadmapItemStarted(_) => "roadmap_item_started",
            Self::RoadmapItemBlocked(_) => "roadmap_item_blocked",
            Self::RoadmapEvidenceAttached(_) => "roadmap_evidence_attached",
            Self::RoadmapItemPassed(_) => "roadmap_item_passed",
            Self::RoadmapItemDropped(_) => "roadmap_item_dropped",
            Self::MemoryRecall(_) => "memory_recall",
            Self::ModelAttempt(_) => "model_attempt",
            Self::ModelRoute(_) => "model_route",
            Self::ModelRouteResult(_) => "model_route_result",
            Self::ModelRetry(_) => "model_retry",
            Self::ModelUsage(_) => "model_usage",
            Self::UsageError(_) => "usage_error",
            Self::CapabilityGeneration(_) => "capability_generation",
            Self::WorkerJob(_) => "worker_job",
            Self::WorkerAttempt(_) => "worker_attempt",
            Self::WorkerLease(_) => "worker_lease",
            Self::WorkerEvent(_) => "worker_event",
            Self::Collaboration(_) => "collaboration",
            Self::Interaction(_) => "interaction",
            Self::TelemetryReference(_) => "telemetry_reference",
            Self::TerminalOutcome(_) => "terminal_outcome",
            Self::RunError(_) => "run_error",
            Self::Advisor(_) => "advisor",
            Self::Agent(_) => "agent",
            Self::AgentState(_) => "agent_state",
            Self::PendingPreview(_) => "pending_preview",
            Self::PendingClaim(_) => "pending_claim",
            Self::PendingApply(_) => "pending_apply",
            Self::PendingDiscard(_) => "pending_discard",
            Self::PendingExpire(_) => "pending_expire",
            Self::ContractViolation(_) => "contract_violation",
            Self::TaskContract(_) => "task_contract",
            Self::TaskReport(_) => "task_report",
        }
    }

    pub(crate) fn data(&self) -> &Value {
        match self {
            Self::Message(v)
            | Self::User(v)
            | Self::Assistant(v)
            | Self::AssistantRaw(v)
            | Self::Final(v)
            | Self::Action(v)
            | Self::ToolCall(v)
            | Self::ToolResult(v)
            | Self::Approval(v)
            | Self::Artifact(v)
            | Self::ContextSnapshot(v)
            | Self::Compaction(v)
            | Self::AutoCompaction(v)
            | Self::AutoCompactionError(v)
            | Self::AutoContinue(v)
            | Self::ToolPrune(v)
            | Self::Handoff(v)
            | Self::Lineage(v)
            | Self::Branch(v)
            | Self::Checkpoint(v)
            | Self::Rewind(v)
            | Self::GoalLifecycle(v)
            | Self::MemoryMutation(v)
            | Self::RoadmapItemCreated(v)
            | Self::RoadmapItemUpdated(v)
            | Self::RoadmapItemStarted(v)
            | Self::RoadmapItemBlocked(v)
            | Self::RoadmapEvidenceAttached(v)
            | Self::RoadmapItemPassed(v)
            | Self::RoadmapItemDropped(v)
            | Self::MemoryRecall(v)
            | Self::ModelAttempt(v)
            | Self::ModelRoute(v)
            | Self::ModelRouteResult(v)
            | Self::ModelRetry(v)
            | Self::ModelUsage(v)
            | Self::UsageError(v)
            | Self::CapabilityGeneration(v)
            | Self::WorkerJob(v)
            | Self::WorkerAttempt(v)
            | Self::WorkerLease(v)
            | Self::WorkerEvent(v)
            | Self::Collaboration(v)
            | Self::Interaction(v)
            | Self::TelemetryReference(v)
            | Self::TerminalOutcome(v)
            | Self::RunError(v)
            | Self::Advisor(v)
            | Self::Agent(v)
            | Self::AgentState(v)
            | Self::PendingPreview(v)
            | Self::PendingClaim(v)
            | Self::PendingApply(v)
            | Self::PendingDiscard(v)
            | Self::PendingExpire(v)
            | Self::ContractViolation(v)
            | Self::TaskContract(v)
            | Self::TaskReport(v) => v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEventV2 {
    pub(crate) event_id: String,
    pub(crate) session_id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) sequence: u64,
    pub(crate) timestamp: String,
    pub(crate) causation_id: Option<String>,
    pub(crate) correlation_id: String,
    pub(crate) schema_version: u32,
    pub(crate) payload: SessionPayloadV2,
    #[serde(default)]
    pub(crate) outbox: Vec<super::outbox::OutboxItem>,
    pub(crate) checksum: String,
}

impl SessionEventV2 {
    pub(crate) fn seal(&mut self) -> Result<(), String> {
        self.checksum.clear();
        self.checksum = checksum(self)?;
        Ok(())
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        if self.schema_version != SESSION_EVENT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported session event schema version {}",
                self.schema_version
            ));
        }
        let mut unsigned = self.clone();
        let expected = std::mem::take(&mut unsigned.checksum);
        let actual = checksum(&unsigned)?;
        if expected != actual {
            return Err(format!("event {} checksum mismatch", self.event_id));
        }
        Ok(())
    }
}

fn checksum(event: &SessionEventV2) -> Result<String, String> {
    let bytes = serde_json::to_vec(event).map_err(|e| e.to_string())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
