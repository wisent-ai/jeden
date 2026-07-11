use super::dataset::{load_dataset, validate_dataset};
use super::graders::{grade, sha256};
use super::metrics::{TerminalReasonV1, ToolStatsV1, UsageMetricsV1};
use super::{
    build_report, canonical_json, CaseExecutionV1, CaseExecutor, EvalCaseV1, EvalReportV1,
    EvalRunner, GraderSpecV1, IsolatedRunV1, RouteEvidenceV1, RunnerConfigV1,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const RECEIPT: &[u8] = b"{\"status\":\"complete\"}\n";
const RECEIPT_DIGEST: &str = "d12df96e8d922d035ca400abb39171428d8596f829f9bad5d707ef1c145e372c";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jeden-eval-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Script {
    Complete,
    MissingArtifact,
    MissingGraderOutput,
    HardGraderFailure,
    ForbiddenAction,
}

struct ScriptedExecutor {
    script: Script,
    calls: usize,
    isolated_runs: Vec<IsolatedRunV1>,
}

impl ScriptedExecutor {
    fn new(script: Script) -> Self {
        Self {
            script,
            calls: 0,
            isolated_runs: Vec::new(),
        }
    }
}

impl CaseExecutor for ScriptedExecutor {
    fn execute(
        &mut self,
        case: &EvalCaseV1,
        isolated: &IsolatedRunV1,
    ) -> Result<CaseExecutionV1, String> {
        self.calls += 1;
        self.isolated_runs.push(isolated.clone());

        if !matches!(self.script, Script::MissingGraderOutput) {
            let marker = if matches!(self.script, Script::HardGraderFailure) {
                format!("{}:not-complete\n", case.id)
            } else {
                format!("{}:complete\n", case.id)
            };
            fs::write(isolated.workspace.join("result.txt"), marker)
                .map_err(|error| error.to_string())?;
        }
        if !matches!(self.script, Script::MissingArtifact) {
            fs::write(isolated.artifacts.join("receipt.json"), RECEIPT)
                .map_err(|error| error.to_string())?;
        }

        let actions = if matches!(self.script, Script::ForbiddenAction) {
            BTreeSet::from(["network.unscoped".to_string()])
        } else {
            BTreeSet::new()
        };
        Ok(CaseExecutionV1 {
            route: RouteEvidenceV1 {
                served_route: "scripted/reference".into(),
                decision: json!({"caseId": case.id, "seed": case.seed, "selected": "scripted/reference"}),
            },
            terminal_reason: TerminalReasonV1::Completed,
            tool_stats: ToolStatsV1 {
                calls: 3,
                failures: 1,
                by_capability: BTreeMap::from([
                    ("artifact.write".into(), 1),
                    ("file.write".into(), 2),
                ]),
            },
            usage: UsageMetricsV1 {
                latency_ms: 17,
                input_tokens: 101,
                output_tokens: 37,
                cost_microunits: 500,
                steps: 4,
            },
            retries: 1,
            failovers: 2,
            memory_reads: 3,
            memory_writes: 4,
            actions,
            hard_violations: Vec::new(),
        })
    }
}

struct RejectExecution;

impl CaseExecutor for RejectExecution {
    fn execute(&mut self, _: &EvalCaseV1, _: &IsolatedRunV1) -> Result<CaseExecutionV1, String> {
        panic!("resumed eval case invoked the executor")
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(option_env!("JEDEN_EVAL_REPOSITORY_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR")))
}

fn reference_manifest() -> PathBuf {
    repository_root().join("benchmarks/manifests/reference-v1.json")
}

fn runner(output_root: &Path) -> EvalRunner {
    EvalRunner::load(RunnerConfigV1 {
        repository_root: repository_root(),
        manifest_path: reference_manifest(),
        output_root: output_root.to_path_buf(),
    })
    .unwrap()
}

fn report(runner: &EvalRunner, outcomes: Vec<super::RunOutcomeV1>) -> EvalReportV1 {
    build_report(runner.manifest_digest().to_string(), outcomes).unwrap()
}

#[test]
fn independent_roots_produce_byte_identical_outcomes_routes_scores_and_reports() {
    let first_root = TempDir::new("determinism-a");
    let second_root = TempDir::new("determinism-b");
    let first_runner = runner(first_root.path());
    let second_runner = runner(second_root.path());
    let mut first_executor = ScriptedExecutor::new(Script::Complete);
    let mut second_executor = ScriptedExecutor::new(Script::Complete);

    let first_outcomes = first_runner.run_all(&mut first_executor).unwrap();
    let second_outcomes = second_runner.run_all(&mut second_executor).unwrap();
    assert_eq!(first_executor.calls, 24);
    assert_eq!(second_executor.calls, 24);
    assert_eq!(first_outcomes, second_outcomes);
    assert!(first_outcomes.iter().all(|outcome| {
        outcome.served_route == "scripted/reference"
            && outcome.terminal_reason == TerminalReasonV1::Completed
            && outcome.deterministic_score.earned == 100
            && outcome.deterministic_score.possible == 100
            && outcome.deterministic_score.passed
    }));

    let first_report = report(&first_runner, first_outcomes);
    let second_report = report(&second_runner, second_outcomes);
    assert_eq!(first_report, second_report);
    assert_eq!(
        canonical_json(&first_report).unwrap(),
        canonical_json(&second_report).unwrap()
    );
}

#[test]
fn rerun_resumes_immutable_outcomes_without_executing_cases_again() {
    let output = TempDir::new("resume");
    let runner = runner(output.path());
    let mut initial = ScriptedExecutor::new(Script::Complete);
    let expected = runner.run_all(&mut initial).unwrap();
    assert_eq!(initial.calls, 24);

    let mut reject = RejectExecution;
    let resumed = runner.run_all(&mut reject).unwrap();
    assert_eq!(resumed, expected);
}

#[test]
fn missing_artifact_and_missing_grader_input_fail_closed() {
    let artifact_root = TempDir::new("missing-artifact");
    let artifact_runner = runner(artifact_root.path());
    let mut missing_artifact = ScriptedExecutor::new(Script::MissingArtifact);
    let artifact_error = artifact_runner.run_all(&mut missing_artifact).unwrap_err();
    assert!(
        artifact_error.contains("missing required artifact receipt.json"),
        "{artifact_error}"
    );

    let grader_root = TempDir::new("missing-grader-input");
    let grader_runner = runner(grader_root.path());
    let mut missing_grader_input = ScriptedExecutor::new(Script::MissingGraderOutput);
    let grader_error = grader_runner
        .run_all(&mut missing_grader_input)
        .unwrap_err();
    assert!(
        grader_error.contains("grader completion-marker missing file"),
        "{grader_error}"
    );
}

#[test]
fn missing_catalog_prevents_runner_from_loading() {
    let temp = TempDir::new("missing-catalog");
    let manifest_path = temp.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(reference_manifest()).unwrap()).unwrap();
    manifest["catalog"] = json!("benchmarks/manifests/does-not-exist.json");
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

    let error = EvalRunner::load(RunnerConfigV1 {
        repository_root: repository_root(),
        manifest_path,
        output_root: temp.path().join("output"),
    })
    .err()
    .expect("a missing catalog must fail runner loading");
    assert!(error.contains("missing catalog"), "{error}");
    assert!(error.contains("does-not-exist.json"), "{error}");
}

#[test]
fn forbidden_actions_and_failed_hard_graders_force_hard_violation() {
    let forbidden_root = TempDir::new("forbidden-action");
    let forbidden_runner = runner(forbidden_root.path());
    let mut forbidden = ScriptedExecutor::new(Script::ForbiddenAction);
    let forbidden_outcome = forbidden_runner.run_all(&mut forbidden).unwrap().remove(0);
    assert_eq!(
        forbidden_outcome.terminal_reason,
        TerminalReasonV1::HardViolation
    );
    assert_eq!(
        forbidden_outcome.hard_violations,
        ["forbidden action: network.unscoped"]
    );
    assert_eq!(forbidden_outcome.deterministic_score.earned, 100);
    assert!(!forbidden_outcome.deterministic_score.passed);
    assert!(!forbidden_outcome.passed());

    let grader_root = TempDir::new("hard-grader");
    let grader_runner = runner(grader_root.path());
    let mut failed_grader = ScriptedExecutor::new(Script::HardGraderFailure);
    let grader_outcome = grader_runner.run_all(&mut failed_grader).unwrap().remove(0);
    assert_eq!(
        grader_outcome.terminal_reason,
        TerminalReasonV1::HardViolation
    );
    assert_eq!(
        grader_outcome.hard_violations,
        ["hard grader failed: completion-marker"]
    );
    assert_eq!(grader_outcome.deterministic_score.earned, 20);
    assert_eq!(grader_outcome.deterministic_score.possible, 100);
    assert!(!grader_outcome.deterministic_score.passed);
    assert!(!grader_outcome.passed());
}

#[test]
fn executor_receives_pairwise_distinct_isolation_paths_beneath_each_run_root() {
    let output = TempDir::new("isolation");
    let runner = runner(output.path());
    let mut executor = ScriptedExecutor::new(Script::Complete);
    runner.run_all(&mut executor).unwrap();

    for isolated in &executor.isolated_runs {
        let paths = [
            ("HOME", &isolated.home),
            ("JEDEN_SESSION_DIR", &isolated.session),
            ("JEDEN_MEMORY_DIR", &isolated.memory),
            ("JEDEN_QUALITY_DB_DIR", &isolated.quality_db),
            ("JEDEN_WORKSPACE", &isolated.workspace),
        ];
        let unique: BTreeSet<_> = paths.iter().map(|(_, path)| path.as_path()).collect();
        assert_eq!(unique.len(), paths.len());
        for (environment_key, path) in paths {
            assert!(
                path.starts_with(&isolated.root),
                "{} escaped {}",
                path.display(),
                isolated.root.display()
            );
            assert_eq!(
                isolated.environment.get(environment_key),
                Some(&path.display().to_string())
            );
        }
        assert!(isolated.root.starts_with(output.path().join("runs")));
    }
}

#[test]
fn canonical_report_commits_every_input_digest_and_aggregate_metric() {
    let output = TempDir::new("report");
    let runner = runner(output.path());
    let mut executor = ScriptedExecutor::new(Script::Complete);
    let outcomes = runner.run_all(&mut executor).unwrap();
    let report = report(&runner, outcomes.clone());

    assert_eq!(
        report.manifest_digest,
        sha256(fs::read(reference_manifest()).unwrap())
    );
    assert_eq!(
        report.dataset_digest,
        sha256(
            fs::read(
                repository_root().join("benchmarks/datasets/jeden-synthetic-reference-v1.json")
            )
            .unwrap()
        )
    );
    assert_eq!(
        report.catalog_digest,
        sha256(
            fs::read(repository_root().join("benchmarks/manifests/scripted-catalog-v1.json"))
                .unwrap()
        )
    );
    assert_eq!(
        report.policy_digest,
        sha256(
            fs::read(repository_root().join("benchmarks/manifests/reference-policy-v1.json"))
                .unwrap()
        )
    );
    assert_eq!(
        report.code_digest,
        "bac7f622513fd50dd27c8c6eab3bf268ea93a48e67e4f46fbe738eb781ffe0b0"
    );
    assert_eq!(
        report.fixture_digests,
        outcomes
            .iter()
            .map(|item| item.fixture_digest.clone())
            .collect()
    );
    assert_eq!(
        report.grader_digests,
        outcomes
            .iter()
            .map(|item| item.grader_digest.clone())
            .collect()
    );
    for outcome in &outcomes {
        let decision = json!({
            "caseId": outcome.case_id,
            "seed": outcome.seed,
            "selected": "scripted/reference",
        });
        assert_eq!(
            outcome.route_decision_digest,
            sha256(serde_json::to_vec(&decision).unwrap())
        );
        assert_eq!(
            outcome.artifacts.get("receipt.json").map(String::as_str),
            Some(RECEIPT_DIGEST)
        );
    }

    assert_eq!(report.case_count, 24);
    assert_eq!(report.passed, 24);
    assert_eq!(report.failed, 0);
    assert_eq!(report.deterministic_points_earned, 2_400);
    assert_eq!(report.deterministic_points_possible, 2_400);
    assert_eq!(report.total_latency_ms, 408);
    assert_eq!(report.total_input_tokens, 2_424);
    assert_eq!(report.total_output_tokens, 888);
    assert_eq!(report.total_cost_microunits, 12_000);
    assert_eq!(report.total_tool_calls, 72);
    assert_eq!(report.total_retries, 24);
    assert_eq!(report.total_failovers, 48);
    assert_eq!(report.total_memory_reads, 72);
    assert_eq!(report.total_memory_writes, 96);
    assert_eq!(report.hard_violation_count, 0);
    assert_eq!(
        report.terminal_reasons,
        BTreeMap::from([("Completed".into(), 24)])
    );

    let canonical = canonical_json(&report).unwrap();
    let decoded: EvalReportV1 = serde_json::from_str(&canonical).unwrap();
    assert_eq!(decoded, report);
    let mut digest_payload = report.clone();
    digest_payload.report_digest.clear();
    assert_eq!(
        report.report_digest,
        sha256(serde_json::to_vec(&digest_payload).unwrap())
    );
}

#[test]
fn process_and_json_schema_graders_emit_scored_success_and_failure_evidence() {
    let isolated = TempDir::new("direct-graders");
    let workspace = isolated.path().join("workspace");
    let artifacts = isolated.path().join("artifacts");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(workspace.join("present.txt"), b"present\n").unwrap();
    let environment = BTreeMap::from([
        (
            "HOME".into(),
            isolated.path().join("home").display().to_string(),
        ),
        ("JEDEN_WORKSPACE".into(), workspace.display().to_string()),
    ]);
    let test_executable = if Path::new("/usr/bin/test").is_file() {
        "/usr/bin/test"
    } else {
        "/bin/test"
    };

    let process_success = GraderSpecV1::Process {
        id: "process-success".into(),
        argv: vec![test_executable.into(), "-f".into(), "present.txt".into()],
        expected_exit: 0,
        stdout_contains: None,
        points: 13,
        hard: true,
    };
    let success = grade(&process_success, &workspace, &artifacts, &environment).unwrap();
    assert_eq!(success.grader_id, "process-success");
    assert_eq!((success.earned, success.possible), (13, 13));
    assert!(success.passed);
    assert!(success.hard);
    assert_eq!(success.detail, "process exit=0, expected=0");

    let process_failure = GraderSpecV1::Process {
        id: "process-failure".into(),
        argv: vec![test_executable.into(), "-f".into(), "absent.txt".into()],
        expected_exit: 0,
        stdout_contains: None,
        points: 17,
        hard: false,
    };
    let failure = grade(&process_failure, &workspace, &artifacts, &environment).unwrap();
    assert_eq!(failure.grader_id, "process-failure");
    assert_eq!((failure.earned, failure.possible), (0, 17));
    assert!(!failure.passed);
    assert!(!failure.hard);
    assert_eq!(failure.detail, "process exit=1, expected=0");

    let schema = json!({
        "type": "object",
        "required": ["status", "attempts"],
        "properties": {
            "status": {"const": "complete"},
            "attempts": {"type": "integer"}
        },
        "additionalProperties": false
    });
    let schema_grader = GraderSpecV1::JsonSchema {
        id: "result-schema".into(),
        path: "result.json".into(),
        schema,
        points: 23,
        hard: true,
    };
    fs::write(
        workspace.join("result.json"),
        br#"{"status":"complete","attempts":2}"#,
    )
    .unwrap();
    let schema_success = grade(&schema_grader, &workspace, &artifacts, &environment).unwrap();
    assert_eq!(schema_success.grader_id, "result-schema");
    assert_eq!((schema_success.earned, schema_success.possible), (23, 23));
    assert!(schema_success.passed);
    assert!(schema_success.hard);
    assert_eq!(schema_success.detail, "JSON Schema match for result.json");

    fs::write(
        workspace.join("result.json"),
        br#"{"status":"queued","attempts":2}"#,
    )
    .unwrap();
    let schema_failure = grade(&schema_grader, &workspace, &artifacts, &environment).unwrap();
    assert_eq!(schema_failure.grader_id, "result-schema");
    assert_eq!((schema_failure.earned, schema_failure.possible), (0, 23));
    assert!(!schema_failure.passed);
    assert!(schema_failure.hard);
    assert_eq!(
        schema_failure.detail,
        "schema mismatch for result.json: $.status does not equal const"
    );
}

#[test]
fn dataset_validation_rejects_a_case_without_a_grader_catalog() {
    let dataset_path =
        repository_root().join("benchmarks/datasets/jeden-synthetic-reference-v1.json");
    let mut dataset = load_dataset(&dataset_path).unwrap();
    assert_eq!(dataset.cases.len(), 24);
    let case = &mut dataset.cases[7];
    let case_id = case.id.clone();
    case.graders.clear();

    let error = validate_dataset(&dataset).unwrap_err();
    assert_eq!(
        error,
        format!("case {case_id} has no deterministic graders")
    );
}
