//! What running one evaluation case produces, and the narrow surface an
//! executor must provide.
//!
//! Split out of `eval/runner.rs`, which had grown past the module line cap.

use super::super::dataset::EvalCaseV1;
use super::super::metrics::{TerminalReasonV1, ToolStatsV1, UsageMetricsV1};
use super::IsolatedRunV1;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteEvidenceV1 {
    pub served_route: String,
    pub decision: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaseExecutionV1 {
    pub route: RouteEvidenceV1,
    pub terminal_reason: TerminalReasonV1,
    pub tool_stats: ToolStatsV1,
    pub usage: UsageMetricsV1,
    pub retries: u32,
    pub failovers: u32,
    pub memory_reads: u32,
    pub memory_writes: u32,
    #[serde(default)]
    pub actions: BTreeSet<String>,
    #[serde(default)]
    pub hard_violations: Vec<String>,
}

pub trait CaseExecutor {
    fn execute(
        &mut self,
        case: &EvalCaseV1,
        isolated: &IsolatedRunV1,
    ) -> Result<CaseExecutionV1, String>;
}
