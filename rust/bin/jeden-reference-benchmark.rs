use jeden::eval::metrics::{TerminalReasonV1, ToolStatsV1, UsageMetricsV1};
use jeden::eval::{
    build_report, canonical_json, CaseExecutionV1, CaseExecutor, EvalCaseV1, EvalManifestV1,
    EvalRunner, IsolatedRunV1, RouteEvidenceV1, RunnerConfigV1,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const RECEIPT: &[u8] = b"{\"status\":\"complete\"}\n";

#[derive(Debug)]
struct Config {
    repository_root: PathBuf,
    manifest_path: PathBuf,
    output_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptedCatalog {
    schema: String,
    revision: String,
    routes: Vec<ScriptedRoute>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptedRoute {
    id: String,
    available: bool,
    capabilities: BTreeSet<String>,
}

struct ReferenceExecutor {
    route: String,
    catalog_revision: String,
}

impl CaseExecutor for ReferenceExecutor {
    fn execute(
        &mut self,
        case: &EvalCaseV1,
        isolated: &IsolatedRunV1,
    ) -> Result<CaseExecutionV1, String> {
        fs::write(
            isolated.workspace.join("result.txt"),
            format!("{}:complete\n", case.id),
        )
        .map_err(|error| format!("cannot write completion marker: {error}"))?;
        fs::write(isolated.artifacts.join("receipt.json"), RECEIPT)
            .map_err(|error| format!("cannot write completion receipt: {error}"))?;

        Ok(CaseExecutionV1 {
            route: RouteEvidenceV1 {
                served_route: self.route.clone(),
                decision: json!({
                    "caseId": case.id,
                    "catalogRevision": self.catalog_revision,
                    "seed": case.seed,
                    "selected": self.route
                }),
            },
            terminal_reason: TerminalReasonV1::Completed,
            tool_stats: ToolStatsV1 {
                calls: 3,
                failures: 0,
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
            failovers: 1,
            memory_reads: 1,
            memory_writes: 1,
            actions: BTreeSet::new(),
            hard_violations: Vec::new(),
        })
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("reference benchmark failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config = parse_args()?;
    let runner = EvalRunner::load(RunnerConfigV1 {
        repository_root: config.repository_root.clone(),
        manifest_path: config.manifest_path.clone(),
        output_root: config.output_root.clone(),
    })?;
    let (route, catalog_revision) =
        load_scripted_route(&config.repository_root, &config.manifest_path)?;
    let mut executor = ReferenceExecutor {
        route,
        catalog_revision,
    };
    let outcomes = runner.run_all(&mut executor)?;
    let report = build_report(runner.manifest_digest().to_string(), outcomes)?;
    let canonical = canonical_json(&report)?;
    write_immutable(
        &config.output_root.join("report.json"),
        canonical.as_bytes(),
    )?;

    if report.case_count == 0 {
        return Err("reference dataset produced no outcomes".into());
    }
    if report.passed != report.case_count || report.failed != 0 || report.hard_violation_count != 0
    {
        return Err(format!(
            "hard benchmark gate failed: passed={}/{} failed={} hardViolations={}",
            report.passed, report.case_count, report.failed, report.hard_violation_count
        ));
    }
    if report.outcomes.iter().any(|outcome| {
        outcome.served_route.trim().is_empty()
            || outcome.usage.input_tokens == 0
            || outcome.usage.output_tokens == 0
            || outcome.usage.latency_ms == 0
            || outcome.grader_evidence.is_empty()
            || outcome.artifacts.is_empty()
    }) {
        return Err("reference evidence lacks route, usage, grader, or artifact evidence".into());
    }

    println!(
        "report={} digest={} cases={} passed={} failed={} points={}/{} retries={} failovers={} toolCalls={} inputTokens={} outputTokens={} costMicrounits={} latencyMs={} hardViolations={}",
        config.output_root.join("report.json").display(),
        report.report_digest,
        report.case_count,
        report.passed,
        report.failed,
        report.deterministic_points_earned,
        report.deterministic_points_possible,
        report.total_retries,
        report.total_failovers,
        report.total_tool_calls,
        report.total_input_tokens,
        report.total_output_tokens,
        report.total_cost_microunits,
        report.total_latency_ms,
        report.hard_violation_count,
    );
    Ok(())
}

fn parse_args() -> Result<Config, String> {
    let mut repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut manifest = PathBuf::from("benchmarks/manifests/reference-v1.json");
    let mut output = PathBuf::from("benchmarks/output/reference-v1");
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = match argument.as_str() {
            "--repository-root" | "--manifest" | "--output-root" => arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a path"))?,
            "--help" | "-h" => {
                println!("usage: jeden-reference-benchmark [--repository-root PATH] [--manifest PATH] [--output-root PATH]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        };
        match argument.as_str() {
            "--repository-root" => repository_root = PathBuf::from(value),
            "--manifest" => manifest = PathBuf::from(value),
            "--output-root" => output = PathBuf::from(value),
            _ => unreachable!(),
        }
    }
    if manifest.is_relative() {
        manifest = repository_root.join(manifest);
    }
    if output.is_relative() {
        output = repository_root.join(output);
    }
    Ok(Config {
        repository_root,
        manifest_path: manifest,
        output_root: output,
    })
}

fn load_scripted_route(
    repository_root: &Path,
    manifest_path: &Path,
) -> Result<(String, String), String> {
    let manifest_bytes = fs::read(manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let manifest: EvalManifestV1 = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid eval manifest: {error}"))?;
    let catalog_path = repository_root.join(&manifest.catalog);
    let catalog_bytes = fs::read(&catalog_path)
        .map_err(|error| format!("cannot read {}: {error}", catalog_path.display()))?;
    let catalog: ScriptedCatalog = serde_json::from_slice(&catalog_bytes)
        .map_err(|error| format!("invalid scripted catalog: {error}"))?;
    if catalog.schema != "jeden.scripted-catalog.v1" {
        return Err(format!(
            "unsupported scripted catalog schema {}",
            catalog.schema
        ));
    }
    let route = catalog
        .routes
        .into_iter()
        .find(|route| route.available && route.capabilities.contains("tools"))
        .ok_or_else(|| "scripted catalog has no available tool-capable route".to_string())?;
    Ok((route.id, catalog.revision))
}

fn write_immutable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    if path.exists() {
        let existing = fs::read(path)
            .map_err(|error| format!("cannot read existing {}: {error}", path.display()))?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(format!("immutable report mismatch at {}", path.display()))
        };
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
