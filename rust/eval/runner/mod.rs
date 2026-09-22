use super::dataset::{EvalCaseV1, load_dataset, load_fixture, safe_relative, validate_sha256};
use super::graders::{grade, sha256, GRADER_IMPLEMENTATION_REVISION};
use super::metrics::{OUTCOME_SCHEMA, RunOutcomeV1, ScoreV1, TerminalReasonV1};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

mod budget;
mod isolation;
mod resume;

use budget::enforce_budget;
use isolation::{atomic_write_new, isolated_run, materialize_fixture, resolve_repo_path, run_key};

pub const MANIFEST_SCHEMA: &str = "jeden.eval-manifest.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalManifestV1 {
    pub schema: String,
    pub dataset: String,
    pub catalog: String,
    pub policy: String,
    pub code_digest: String,
}

#[derive(Clone, Debug)]
pub struct RunnerConfigV1 {
    pub repository_root: PathBuf,
    pub manifest_path: PathBuf,
    pub output_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct IsolatedRunV1 {
    pub run_key: String,
    pub root: PathBuf,
    pub home: PathBuf,
    pub session: PathBuf,
    pub memory: PathBuf,
    pub quality_db: PathBuf,
    pub workspace: PathBuf,
    pub artifacts: PathBuf,
    pub environment: BTreeMap<String, String>,
}

mod execution;

pub use execution::{CaseExecutionV1, CaseExecutor, RouteEvidenceV1};


pub struct EvalRunner {
    pub(super) config: RunnerConfigV1,
    pub(super) manifest: EvalManifestV1,
    pub(super) manifest_digest: String,
    pub(super) dataset_bytes: Vec<u8>,
    pub(super) catalog_digest: String,
    pub(super) policy_digest: String,
}

impl EvalRunner {
    pub fn load(config: RunnerConfigV1) -> Result<Self, String> {
        let repository_root = config.repository_root.canonicalize().map_err(|error| {
            format!(
                "cannot resolve repository root {}: {error}",
                config.repository_root.display()
            )
        })?;
        let config = RunnerConfigV1 {
            repository_root,
            ..config
        };
        let manifest_bytes = fs::read(&config.manifest_path).map_err(|error| {
            format!(
                "missing eval manifest {}: {error}",
                config.manifest_path.display()
            )
        })?;
        let manifest: EvalManifestV1 = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| format!("invalid eval manifest: {error}"))?;
        if manifest.schema != MANIFEST_SCHEMA {
            return Err(format!(
                "unsupported eval manifest schema {}",
                manifest.schema
            ));
        }
        validate_sha256(&manifest.code_digest)?;
        let dataset_path = resolve_repo_path(&config.repository_root, &manifest.dataset)?;
        let catalog_path = resolve_repo_path(&config.repository_root, &manifest.catalog)?;
        let policy_path = resolve_repo_path(&config.repository_root, &manifest.policy)?;
        let dataset_bytes = fs::read(&dataset_path)
            .map_err(|error| format!("missing dataset {}: {error}", dataset_path.display()))?;
        let catalog = fs::read(&catalog_path)
            .map_err(|error| format!("missing catalog {}: {error}", catalog_path.display()))?;
        let policy = fs::read(&policy_path)
            .map_err(|error| format!("missing policy {}: {error}", policy_path.display()))?;
        serde_json::from_slice::<serde_json::Value>(&catalog)
            .map_err(|error| format!("invalid catalog artifact: {error}"))?;
        serde_json::from_slice::<serde_json::Value>(&policy)
            .map_err(|error| format!("invalid policy artifact: {error}"))?;
        let _ = load_dataset(&dataset_path)?;
        Ok(Self {
            config,
            manifest,
            manifest_digest: sha256(&manifest_bytes),
            dataset_bytes,
            catalog_digest: sha256(catalog),
            policy_digest: sha256(policy),
        })
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn run_all(&self, executor: &mut dyn CaseExecutor) -> Result<Vec<RunOutcomeV1>, String> {
        let dataset_path = resolve_repo_path(&self.config.repository_root, &self.manifest.dataset)?;
        let dataset = load_dataset(&dataset_path)?;
        dataset
            .cases
            .iter()
            .map(|case| self.run_case(case, executor))
            .collect()
    }

    pub fn run_case(
        &self,
        case: &EvalCaseV1,
        executor: &mut dyn CaseExecutor,
    ) -> Result<RunOutcomeV1, String> {
        let dataset_digest = sha256(&self.dataset_bytes);
        let fixture_path = resolve_repo_path(&self.config.repository_root, &case.fixture)?;
        let fixture_bytes = fs::read(&fixture_path)
            .map_err(|error| format!("missing fixture {}: {error}", fixture_path.display()))?;
        let fixture = load_fixture(&fixture_path)?;
        let fixture_digest = sha256(&fixture_bytes);
        let grader_bytes = serde_json::to_vec(&(GRADER_IMPLEMENTATION_REVISION, &case.graders))
            .map_err(|error| error.to_string())?;
        let grader_digest = sha256(grader_bytes);
        let run_key = run_key(
            case,
            &dataset_digest,
            &fixture_digest,
            &grader_digest,
            &self.manifest.code_digest,
            &self.catalog_digest,
            &self.policy_digest,
        );
        let isolated = isolated_run(&self.config.output_root, &run_key)?;
        let outcome_path = isolated.root.join("outcome.json");
        if outcome_path.exists() {
            let outcome: RunOutcomeV1 = serde_json::from_slice(
                &fs::read(&outcome_path).map_err(|error| error.to_string())?,
            )
            .map_err(|error| {
                format!(
                    "invalid resumable outcome {}: {error}",
                    outcome_path.display()
                )
            })?;
            self.validate_resumed(
                &outcome,
                case,
                &run_key,
                &fixture_digest,
                &grader_digest,
                &isolated,
            )?;
            return Ok(outcome);
        }
        materialize_fixture(&fixture, &isolated.workspace)?;
        let execution = executor
            .execute(case, &isolated)
            .map_err(|error| format!("case {} execution failed: {error}", case.id))?;
        if execution.route.served_route.trim().is_empty() {
            return Err(format!(
                "case {} executor returned no served route",
                case.id
            ));
        }
        let route_decision_digest = sha256(
            serde_json::to_vec(&execution.route.decision).map_err(|error| error.to_string())?,
        );
        let mut hard_violations = execution.hard_violations;
        for action in execution.actions.intersection(&case.forbidden_actions) {
            hard_violations.push(format!("forbidden action: {action}"));
        }
        enforce_budget(
            case,
            &execution.usage,
            &execution.tool_stats,
            &mut hard_violations,
        );
        let mut artifacts = BTreeMap::new();
        for expected in &case.expected_artifacts {
            let path = isolated.artifacts.join(safe_relative(&expected.path)?);
            let bytes = fs::read(&path).map_err(|error| {
                format!(
                    "case {} missing required artifact {}: {error}",
                    case.id, expected.path
                )
            })?;
            let actual = sha256(&bytes);
            if actual != expected.sha256 {
                hard_violations.push(format!("artifact {} digest mismatch", expected.path));
            }
            artifacts.insert(expected.path.clone(), actual);
        }
        let mut grader_evidence = Vec::with_capacity(case.graders.len());
        for spec in &case.graders {
            let evidence = grade(
                spec,
                &isolated.workspace,
                &isolated.artifacts,
                &isolated.environment,
            )?;
            if evidence.hard && !evidence.passed {
                hard_violations.push(format!("hard grader failed: {}", evidence.grader_id));
            }
            grader_evidence.push(evidence);
        }
        let earned = grader_evidence.iter().map(|item| item.earned).sum();
        let possible = grader_evidence.iter().map(|item| item.possible).sum();
        let deterministic_score = ScoreV1 {
            earned,
            possible,
            passed: earned == possible && hard_violations.is_empty(),
        };
        hard_violations.sort();
        hard_violations.dedup();
        let terminal_reason = if hard_violations.is_empty() {
            execution.terminal_reason
        } else {
            TerminalReasonV1::HardViolation
        };
        let outcome = RunOutcomeV1 {
            schema: OUTCOME_SCHEMA.into(),
            run_key,
            case_id: case.id.clone(),
            seed: case.seed,
            dataset_digest,
            fixture_digest,
            grader_digest,
            code_digest: self.manifest.code_digest.clone(),
            catalog_digest: self.catalog_digest.clone(),
            policy_digest: self.policy_digest.clone(),
            served_route: execution.route.served_route,
            route_decision_digest,
            terminal_reason,
            deterministic_score,
            grader_evidence,
            tool_stats: execution.tool_stats,
            usage: execution.usage,
            retries: execution.retries,
            failovers: execution.failovers,
            memory_reads: execution.memory_reads,
            memory_writes: execution.memory_writes,
            hard_violations,
            artifacts,
        };
        atomic_write_new(
            &outcome_path,
            &serde_json::to_vec(&outcome).map_err(|error| error.to_string())?,
        )?;
        Ok(outcome)
    }

}
