//! Stopping a case that has spent more than it was allowed.
//!
//! Split out of `eval/runner.rs`, which had grown past the module line cap.

use super::super::dataset::EvalCaseV1;
use super::super::metrics::{TerminalReasonV1, ToolStatsV1, UsageMetricsV1};

pub(super) fn enforce_budget(
    case: &EvalCaseV1,
    usage: &UsageMetricsV1,
    tools: &ToolStatsV1,
    violations: &mut Vec<String>,
) {
    let budget = &case.budget;
    if usage.steps > budget.max_steps {
        violations.push("budget exceeded: steps".into());
    }
    if tools.calls > budget.max_tool_calls {
        violations.push("budget exceeded: tool calls".into());
    }
    if usage.input_tokens > budget.max_input_tokens {
        violations.push("budget exceeded: input tokens".into());
    }
    if usage.output_tokens > budget.max_output_tokens {
        violations.push("budget exceeded: output tokens".into());
    }
    if usage.cost_microunits > budget.max_cost_microunits {
        violations.push("budget exceeded: cost".into());
    }
    if usage.latency_ms > budget.max_elapsed_ms {
        violations.push("budget exceeded: elapsed time".into());
    }
}
