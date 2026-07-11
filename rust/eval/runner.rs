use super::dataset::{
    load_dataset, load_fixture, safe_relative, validate_sha256, EvalCaseV1, FixtureV1,
};
use super::graders::{grade, sha256, GRADER_IMPLEMENTATION_REVISION};
use super::metrics::{
    RunOutcomeV1, ScoreV1, TerminalReasonV1, ToolStatsV1, UsageMetricsV1, OUTCOME_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

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

pub struct EvalRunner {
    config: RunnerConfigV1,
    manifest: EvalManifestV1,
    manifest_digest: String,
    dataset_bytes: Vec<u8>,
    catalog_digest: String,
    policy_digest: String,
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

    fn validate_resumed(
        &self,
        outcome: &RunOutcomeV1,
        case: &EvalCaseV1,
        run_key: &str,
        fixture_digest: &str,
        grader_digest: &str,
        isolated: &IsolatedRunV1,
    ) -> Result<(), String> {
        if outcome.schema != OUTCOME_SCHEMA
            || outcome.run_key != run_key
            || outcome.case_id != case.id
            || outcome.seed != case.seed
            || outcome.dataset_digest != sha256(&self.dataset_bytes)
            || outcome.fixture_digest != fixture_digest
            || outcome.grader_digest != grader_digest
            || outcome.code_digest != self.manifest.code_digest
            || outcome.catalog_digest != self.catalog_digest
            || outcome.policy_digest != self.policy_digest
        {
            return Err(format!(
                "resumable outcome for {} does not match immutable run inputs",
                case.id
            ));
        }
        for expected in &case.expected_artifacts {
            let bytes = fs::read(isolated.artifacts.join(safe_relative(&expected.path)?)).map_err(
                |error| {
                    format!(
                        "resumable outcome missing artifact {}: {error}",
                        expected.path
                    )
                },
            )?;
            if sha256(bytes) != expected.sha256 {
                return Err(format!(
                    "resumable artifact {} digest mismatch",
                    expected.path
                ));
            }
        }
        Ok(())
    }
}

fn resolve_repo_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let candidate = root.join(safe_relative(relative)?);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "cannot resolve repository artifact {}: {error}",
            candidate.display()
        )
    })?;
    if !canonical.starts_with(root) {
        return Err(format!("repository artifact escapes root: {relative}"));
    }
    Ok(canonical)
}

fn run_key(
    case: &EvalCaseV1,
    dataset: &str,
    fixture: &str,
    grader: &str,
    code: &str,
    catalog: &str,
    policy: &str,
) -> String {
    let mut digest = Sha256::new();
    for part in [
        case.id.as_str(),
        &case.seed.to_string(),
        dataset,
        fixture,
        grader,
        code,
        catalog,
        policy,
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn isolated_run(output_root: &Path, run_key: &str) -> Result<IsolatedRunV1, String> {
    let root = output_root.join("runs").join(run_key);
    let home = root.join("home");
    let session = root.join("session");
    let memory = root.join("memory");
    let quality_db = root.join("quality");
    let workspace = root.join("workspace");
    let artifacts = root.join("artifacts");
    for path in [
        &home,
        &session,
        &memory,
        &quality_db,
        &workspace,
        &artifacts,
    ] {
        fs::create_dir_all(path)
            .map_err(|error| format!("cannot create isolated path {}: {error}", path.display()))?;
    }
    let environment = BTreeMap::from([
        ("HOME".into(), home.display().to_string()),
        ("JEDEN_SESSION_DIR".into(), session.display().to_string()),
        ("JEDEN_MEMORY_DIR".into(), memory.display().to_string()),
        (
            "JEDEN_QUALITY_DB_DIR".into(),
            quality_db.display().to_string(),
        ),
        ("JEDEN_WORKSPACE".into(), workspace.display().to_string()),
        ("JEDEN_ARTIFACT_DIR".into(), artifacts.display().to_string()),
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("TZ".into(), "UTC".into()),
        ("LANG".into(), "C".into()),
    ]);
    Ok(IsolatedRunV1 {
        run_key: run_key.into(),
        root,
        home,
        session,
        memory,
        quality_db,
        workspace,
        artifacts,
        environment,
    })
}

fn materialize_fixture(fixture: &FixtureV1, workspace: &Path) -> Result<(), String> {
    for (relative, content) in &fixture.files {
        let target = workspace.join(safe_relative(relative)?);
        if target.exists() {
            let existing = fs::read(&target).map_err(|error| error.to_string())?;
            if existing != content.as_bytes() {
                return Err(format!(
                    "partially materialized fixture differs at {relative}"
                ));
            }
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        atomic_write_new(&target, content.as_bytes())?;
    }
    Ok(())
}

fn enforce_budget(
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

fn atomic_write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
