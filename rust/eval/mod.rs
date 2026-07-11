pub mod dataset;
pub mod graders;
pub mod metrics;
pub mod report;
pub mod runner;

#[cfg(test)]
mod tests;

pub use dataset::{EvalBudgetV1, EvalCaseV1, EvalDatasetV1, ExpectedArtifactV1, GraderSpecV1};
pub use metrics::{RunOutcomeV1, TerminalReasonV1};
pub use report::{build_report, canonical_json, EvalReportV1};
pub use runner::{
    CaseExecutionV1, CaseExecutor, EvalManifestV1, EvalRunner, IsolatedRunV1, RouteEvidenceV1,
    RunnerConfigV1,
};
