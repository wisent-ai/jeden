use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const OUTCOME_SCHEMA: &str = "jeden.run-outcome.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunOutcomeV1 {
    pub schema: String,
    pub run_key: String,
    pub case_id: String,
    pub seed: u64,
    pub dataset_digest: String,
    pub fixture_digest: String,
    pub grader_digest: String,
    pub code_digest: String,
    pub catalog_digest: String,
    pub policy_digest: String,
    pub served_route: String,
    pub route_decision_digest: String,
    pub terminal_reason: TerminalReasonV1,
    pub deterministic_score: ScoreV1,
    pub grader_evidence: Vec<GraderEvidenceV1>,
    pub tool_stats: ToolStatsV1,
    pub usage: UsageMetricsV1,
    pub retries: u32,
    pub failovers: u32,
    pub memory_reads: u32,
    pub memory_writes: u32,
    pub hard_violations: Vec<String>,
    pub artifacts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TerminalReasonV1 {
    Completed,
    AgentFailed,
    BudgetExceeded,
    Cancelled,
    InfrastructureFailed,
    HardViolation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreV1 {
    pub earned: u32,
    pub possible: u32,
    pub passed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraderEvidenceV1 {
    pub grader_id: String,
    pub grader_digest: String,
    pub earned: u32,
    pub possible: u32,
    pub passed: bool,
    pub hard: bool,
    pub evidence_digest: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolStatsV1 {
    pub calls: u32,
    pub failures: u32,
    pub by_capability: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageMetricsV1 {
    pub latency_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_microunits: u64,
    pub steps: u32,
}

impl RunOutcomeV1 {
    pub fn passed(&self) -> bool {
        self.hard_violations.is_empty()
            && self.deterministic_score.passed
            && self.terminal_reason == TerminalReasonV1::Completed
    }
}
