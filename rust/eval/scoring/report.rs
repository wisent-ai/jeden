use super::graders::sha256;
use super::metrics::RunOutcomeV1;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const REPORT_SCHEMA: &str = "jeden.eval-report.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalReportV1 {
    pub schema: String,
    pub manifest_digest: String,
    pub dataset_digest: String,
    pub code_digest: String,
    pub catalog_digest: String,
    pub policy_digest: String,
    pub fixture_digests: BTreeSet<String>,
    pub grader_digests: BTreeSet<String>,
    pub case_count: u32,
    pub passed: u32,
    pub failed: u32,
    pub deterministic_points_earned: u64,
    pub deterministic_points_possible: u64,
    pub total_latency_ms: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_microunits: u64,
    pub total_tool_calls: u64,
    pub total_retries: u64,
    pub total_failovers: u64,
    pub total_memory_reads: u64,
    pub total_memory_writes: u64,
    pub hard_violation_count: u64,
    pub terminal_reasons: BTreeMap<String, u32>,
    pub outcomes: Vec<RunOutcomeV1>,
    pub report_digest: String,
}

pub fn build_report(
    manifest_digest: String,
    mut outcomes: Vec<RunOutcomeV1>,
) -> Result<EvalReportV1, String> {
    if outcomes.is_empty() {
        return Err("cannot report an empty eval run".into());
    }
    outcomes.sort_by(|left, right| left.case_id.cmp(&right.case_id));
    let first = &outcomes[0];
    for outcome in &outcomes {
        if outcome.dataset_digest != first.dataset_digest
            || outcome.code_digest != first.code_digest
            || outcome.catalog_digest != first.catalog_digest
            || outcome.policy_digest != first.policy_digest
        {
            return Err(format!(
                "outcome {} has report-incompatible digests",
                outcome.case_id
            ));
        }
    }
    let mut terminal_reasons = BTreeMap::new();
    for outcome in &outcomes {
        *terminal_reasons
            .entry(format!("{:?}", outcome.terminal_reason))
            .or_insert(0) += 1;
    }
    let mut report = EvalReportV1 {
        schema: REPORT_SCHEMA.into(),
        manifest_digest,
        dataset_digest: first.dataset_digest.clone(),
        code_digest: first.code_digest.clone(),
        catalog_digest: first.catalog_digest.clone(),
        policy_digest: first.policy_digest.clone(),
        fixture_digests: outcomes
            .iter()
            .map(|item| item.fixture_digest.clone())
            .collect(),
        grader_digests: outcomes
            .iter()
            .map(|item| item.grader_digest.clone())
            .collect(),
        case_count: outcomes.len() as u32,
        passed: outcomes.iter().filter(|item| item.passed()).count() as u32,
        failed: outcomes.iter().filter(|item| !item.passed()).count() as u32,
        deterministic_points_earned: outcomes
            .iter()
            .map(|item| u64::from(item.deterministic_score.earned))
            .sum(),
        deterministic_points_possible: outcomes
            .iter()
            .map(|item| u64::from(item.deterministic_score.possible))
            .sum(),
        total_latency_ms: outcomes.iter().map(|item| item.usage.latency_ms).sum(),
        total_input_tokens: outcomes.iter().map(|item| item.usage.input_tokens).sum(),
        total_output_tokens: outcomes.iter().map(|item| item.usage.output_tokens).sum(),
        total_cost_microunits: outcomes.iter().map(|item| item.usage.cost_microunits).sum(),
        total_tool_calls: outcomes
            .iter()
            .map(|item| u64::from(item.tool_stats.calls))
            .sum(),
        total_retries: outcomes.iter().map(|item| u64::from(item.retries)).sum(),
        total_failovers: outcomes.iter().map(|item| u64::from(item.failovers)).sum(),
        total_memory_reads: outcomes
            .iter()
            .map(|item| u64::from(item.memory_reads))
            .sum(),
        total_memory_writes: outcomes
            .iter()
            .map(|item| u64::from(item.memory_writes))
            .sum(),
        hard_violation_count: outcomes
            .iter()
            .map(|item| item.hard_violations.len() as u64)
            .sum(),
        terminal_reasons,
        outcomes,
        report_digest: String::new(),
    };
    report.report_digest = sha256(serde_json::to_vec(&report).map_err(|error| error.to_string())?);
    Ok(report)
}

pub fn canonical_json(report: &EvalReportV1) -> Result<String, String> {
    let expected = {
        let mut copy = report.clone();
        copy.report_digest.clear();
        sha256(serde_json::to_vec(&copy).map_err(|error| error.to_string())?)
    };
    if expected != report.report_digest {
        return Err("eval report digest does not match canonical content".into());
    }
    serde_json::to_string(report)
        .map(|text| text + "\n")
        .map_err(|error| error.to_string())
}
