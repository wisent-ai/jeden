//! The closed vocabulary of what a session event can say: a producer cannot
//! persist a kind that is not a variant here.
//!
//! Split out of `session/event.rs`, which had grown past the module line cap.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    CompletionState(Value),
    CompletionReview(Value),
    CompletionRejected(Value),
    AssistantMessage(Value),
}
