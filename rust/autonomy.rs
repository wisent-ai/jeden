use crate::agent::{Conversation, RunHooks};
use crate::protocol::extract_json_object;
use crate::Args;
use rand::{distributions::Alphanumeric, Rng};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CONTRACT_REVIEW_ROUNDS: usize = 3;
const EXECUTION_REVIEW_ROUNDS: usize = 5;
const STRUCTURED_OUTPUT_ATTEMPTS: usize = 3;
const PREFERENCE_MATCH_LIMIT: usize = 30;
const PREFERENCE_TEXT_LIMIT: usize = 700;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceFact {
    source: String,
    fact: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptanceCriterion {
    id: String,
    outcome: String,
    observation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceRequirement {
    criterion_id: String,
    kind: String,
    observation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskContract {
    schema_version: u32,
    title: String,
    rough_objective: String,
    intended_user: String,
    intended_outcome: String,
    starting_state: Vec<EvidenceFact>,
    sources_of_truth: Vec<EvidenceFact>,
    constraints: Vec<String>,
    non_goals: Vec<String>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
    rejection_conditions: Vec<String>,
    required_evidence: Vec<EvidenceRequirement>,
    implementation_plan: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContractGate {
    schema_version: u32,
    decision: String,
    summary: String,
    issues: Vec<String>,
    strengths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CriterionVerdict {
    id: String,
    passed: bool,
    evidence: Vec<String>,
    gap: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskVerdict {
    schema_version: u32,
    decision: String,
    summary: String,
    criteria: Vec<CriterionVerdict>,
    contradictions: Vec<String>,
    required_repairs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferenceMatch {
    query: String,
    timestamp: String,
    runtime: String,
    session_id: String,
    event_type: String,
    text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferenceEvidence {
    local_profiles: Vec<EvidenceFact>,
    transcript_matches: Vec<PreferenceMatch>,
    unavailable_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunReceipt {
    schema_version: u32,
    run_id: String,
    state: String,
    rough_objective: String,
    started_at: String,
    completed_at: String,
    run_directory: PathBuf,
    contract_path: PathBuf,
    verdict_path: Option<PathBuf>,
    preference_evidence_path: PathBuf,
    planner_session: PathBuf,
    contract_review_sessions: Vec<PathBuf>,
    execution_session: Option<PathBuf>,
    acceptance_review_sessions: Vec<PathBuf>,
    execution_rounds: usize,
    error: Option<String>,
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let objective = args.positionals.join(" ");
    if objective.trim().is_empty() {
        return Err("pursue requires a rough objective".into());
    }

    let started_at = crate::agent::now_stamp();
    let run_id = run_id();
    let run_directory = args.cwd.join(".jeden/autonomy").join(&run_id);
    fs::create_dir_all(&run_directory).map_err(|error| error.to_string())?;

    let preference_evidence =
        collect_preference_evidence(&args.cwd, &objective, args.allow_command || args.yolo);
    let preference_evidence_path = run_directory.join("preference-evidence.json");
    write_json(&preference_evidence_path, &preference_evidence)?;
    let preference_json =
        serde_json::to_string_pretty(&preference_evidence).map_err(|error| error.to_string())?;

    let mut read_only_args = args.clone();
    read_only_args.command = "run".into();
    read_only_args.positionals.clear();
    read_only_args.allow_write = false;
    read_only_args.allow_command = false;
    read_only_args.yolo = false;
    read_only_args.goal = None;
    read_only_args.autonomous = true;

    let mut planner = Conversation::new(&args.cwd)?;
    let mut hooks = RunHooks::inert();
    let contract_prompt = contract_prompt(&objective, &preference_json);
    let mut contract: TaskContract = run_checked_json(
        &mut planner,
        &read_only_args,
        &contract_prompt,
        &mut hooks,
        "task contract",
        |candidate| validate_contract(candidate, &objective),
    )?;

    let mut contract_review_sessions = Vec::new();
    let mut accepted_contract = false;
    for round in 0..CONTRACT_REVIEW_ROUNDS {
        let contract_json =
            serde_json::to_string_pretty(&contract).map_err(|error| error.to_string())?;
        let mut reviewer = Conversation::new(&args.cwd)?;
        let review_prompt = contract_review_prompt(&objective, &contract_json, &preference_json);
        let gate: ContractGate = run_checked_json(
            &mut reviewer,
            &read_only_args,
            &review_prompt,
            &mut hooks,
            "contract review",
            validate_contract_gate,
        )?;
        contract_review_sessions.push(reviewer.session_path());
        write_json(
            &run_directory.join(format!("contract-review-{}.json", round + 1)),
            &gate,
        )?;
        if gate.decision == "accept" && gate.issues.is_empty() {
            accepted_contract = true;
            break;
        }
        if round + 1 == CONTRACT_REVIEW_ROUNDS {
            break;
        }
        let revision_prompt = contract_revision_prompt(&objective, &contract_json, &gate);
        contract = run_checked_json(
            &mut planner,
            &read_only_args,
            &revision_prompt,
            &mut hooks,
            "revised task contract",
            |candidate| validate_contract(candidate, &objective),
        )?;
    }

    let contract_path = run_directory.join("contract.json");
    write_json(&contract_path, &contract)?;
    let planner_session = planner.session_path();
    if !accepted_contract {
        let error = "task contract did not pass independent review".to_string();
        let receipt = RunReceipt {
            schema_version: 1,
            run_id,
            state: "contract_rejected".into(),
            rough_objective: objective,
            started_at,
            completed_at: crate::agent::now_stamp(),
            run_directory: run_directory.clone(),
            contract_path,
            verdict_path: None,
            preference_evidence_path,
            planner_session,
            contract_review_sessions,
            execution_session: None,
            acceptance_review_sessions: Vec::new(),
            execution_rounds: 0,
            error: Some(error.clone()),
        };
        write_json(&run_directory.join("receipt.json"), &receipt)?;
        return Err(format!("{}; receipt: {}", error, run_directory.display()));
    }

    let contract_json =
        serde_json::to_string_pretty(&contract).map_err(|error| error.to_string())?;
    let mut execution_args = args.clone();
    execution_args.command = "run".into();
    execution_args.positionals.clear();
    execution_args.goal = Some(contract.intended_outcome.clone());
    execution_args.autonomous = true;

    let mut executor = Conversation::new(&args.cwd)?;
    let mut execution_text = executor.run_turn(
        &execution_args,
        &execution_prompt(&contract_json, &run_directory),
        &[],
        &mut hooks,
    )?;
    write_text(&run_directory.join("execution-1.md"), &execution_text)?;

    let mut acceptance_review_sessions = Vec::new();
    let mut final_verdict = None;
    let mut execution_rounds = 1usize;
    for round in 0..EXECUTION_REVIEW_ROUNDS {
        let mut reviewer = Conversation::new(&args.cwd)?;
        let review_prompt = acceptance_review_prompt(&contract_json, &execution_text);
        let verdict: TaskVerdict = run_checked_json(
            &mut reviewer,
            &read_only_args,
            &review_prompt,
            &mut hooks,
            "acceptance verdict",
            |candidate| validate_verdict(candidate, &contract),
        )?;
        acceptance_review_sessions.push(reviewer.session_path());
        let round_verdict_path = run_directory.join(format!("verdict-{}.json", round + 1));
        write_json(&round_verdict_path, &verdict)?;
        if verdict_accepted(&verdict) {
            final_verdict = Some(verdict);
            break;
        }
        if round + 1 == EXECUTION_REVIEW_ROUNDS {
            final_verdict = Some(verdict);
            break;
        }
        execution_text = executor.run_turn(
            &execution_args,
            &repair_prompt(&contract_json, &verdict),
            &[],
            &mut hooks,
        )?;
        execution_rounds += 1;
        write_text(
            &run_directory.join(format!("execution-{}.md", execution_rounds)),
            &execution_text,
        )?;
    }

    let verdict = final_verdict.ok_or("acceptance review produced no verdict")?;
    let verdict_path = run_directory.join("verdict.json");
    write_json(&verdict_path, &verdict)?;
    let accepted = verdict_accepted(&verdict);
    let state = if accepted { "succeeded" } else { "rejected" };
    let error = (!accepted).then(|| "execution did not satisfy the approved contract".to_string());
    let receipt = RunReceipt {
        schema_version: 1,
        run_id: run_id.clone(),
        state: state.into(),
        rough_objective: objective,
        started_at,
        completed_at: crate::agent::now_stamp(),
        run_directory: run_directory.clone(),
        contract_path,
        verdict_path: Some(verdict_path),
        preference_evidence_path,
        planner_session,
        contract_review_sessions,
        execution_session: Some(executor.session_path()),
        acceptance_review_sessions,
        execution_rounds,
        error: error.clone(),
    };
    let receipt_path = run_directory.join("receipt.json");
    write_json(&receipt_path, &receipt)?;

    if let Some(error) = error {
        return Err(format!("{}; receipt: {}", error, receipt_path.display()));
    }
    if args.json {
        return serde_json::to_string_pretty(&json!({
            "ok": true,
            "runId": run_id,
            "runDirectory": run_directory,
            "contract": receipt.contract_path,
            "verdict": receipt.verdict_path,
            "receipt": receipt_path,
            "summary": verdict.summary,
        }))
        .map(|text| text + "\n")
        .map_err(|error| error.to_string());
    }
    Ok(format!(
        "{}\ncontract: {}\nverdict: {}\nreceipt: {}\n",
        verdict.summary,
        receipt.contract_path.display(),
        receipt
            .verdict_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        receipt_path.display(),
    ))
}

fn parse_json_response<T: DeserializeOwned>(text: &str, label: &str) -> Result<T, String> {
    let json_text =
        extract_json_object(text).map_err(|error| format!("invalid {label}: {error}"))?;
    serde_json::from_str(json_text).map_err(|error| format!("invalid {label}: {error}"))
}

fn run_checked_json<T>(
    conversation: &mut Conversation,
    args: &Args,
    initial_prompt: &str,
    hooks: &mut RunHooks<'_>,
    label: &str,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let mut prompt = initial_prompt.to_string();
    let mut last_error = String::new();
    for attempt in 0..STRUCTURED_OUTPUT_ATTEMPTS {
        let response = conversation.run_turn(args, &prompt, &[], hooks)?;
        match parse_json_response(&response, label).and_then(|value| {
            validate(&value)?;
            Ok(value)
        }) {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = error;
                if attempt + 1 < STRUCTURED_OUTPUT_ATTEMPTS {
                    prompt = format!(
                        "Your previous response was invalid: {}.\nReturn a corrected complete JSON object only. Do not add prose or change the assigned role.\n\nOriginal request:\n{}",
                        last_error, initial_prompt
                    );
                }
            }
        }
    }
    Err(format!(
        "{label} remained invalid after {STRUCTURED_OUTPUT_ATTEMPTS} attempts: {last_error}"
    ))
}

fn validate_contract(contract: &TaskContract, objective: &str) -> Result<(), String> {
    if contract.schema_version != 1 {
        return Err("task contract schemaVersion must be 1".into());
    }
    if contract.rough_objective != objective {
        return Err("task contract roughObjective must preserve the exact input objective".into());
    }
    require_text("title", &contract.title)?;
    require_text("intendedUser", &contract.intended_user)?;
    require_text("intendedOutcome", &contract.intended_outcome)?;
    for fact in contract
        .starting_state
        .iter()
        .chain(contract.sources_of_truth.iter())
    {
        require_text("evidence source", &fact.source)?;
        require_text("evidence fact", &fact.fact)?;
    }
    validate_texts("constraint", &contract.constraints)?;
    validate_texts("non-goal", &contract.non_goals)?;
    validate_texts("rejection condition", &contract.rejection_conditions)?;
    validate_texts("implementation step", &contract.implementation_plan)?;
    if contract.starting_state.is_empty()
        || contract.sources_of_truth.is_empty()
        || contract.acceptance_criteria.is_empty()
        || contract.rejection_conditions.is_empty()
        || contract.required_evidence.is_empty()
        || contract.implementation_plan.is_empty()
    {
        return Err("task contract is missing a required non-empty decision set".into());
    }

    let mut ids = BTreeSet::new();
    for criterion in &contract.acceptance_criteria {
        validate_id(&criterion.id)?;
        require_text("acceptance outcome", &criterion.outcome)?;
        require_text("acceptance observation", &criterion.observation)?;
        if !ids.insert(criterion.id.as_str()) {
            return Err(format!(
                "duplicate acceptance criterion id: {}",
                criterion.id
            ));
        }
    }

    const EVIDENCE_KINDS: &[&str] = &[
        "source",
        "configuration",
        "manifest",
        "history",
        "product-output",
        "artifact",
    ];
    let mut evidence_ids = BTreeSet::new();
    for evidence in &contract.required_evidence {
        if !ids.contains(evidence.criterion_id.as_str()) {
            return Err(format!(
                "evidence references unknown criterion: {}",
                evidence.criterion_id
            ));
        }
        if !EVIDENCE_KINDS.contains(&evidence.kind.as_str()) {
            return Err(format!("unsupported evidence kind: {}", evidence.kind));
        }
        require_text("evidence observation", &evidence.observation)?;
        evidence_ids.insert(evidence.criterion_id.as_str());
    }
    for id in ids {
        if !evidence_ids.contains(id) {
            return Err(format!("criterion has no required evidence: {id}"));
        }
    }
    Ok(())
}
fn validate_contract_gate(gate: &ContractGate) -> Result<(), String> {
    if gate.schema_version != 1 || !matches!(gate.decision.as_str(), "accept" | "revise") {
        return Err("contract review has an invalid schemaVersion or decision".into());
    }
    require_text("contract review summary", &gate.summary)?;
    if gate.decision == "accept" && !gate.issues.is_empty() {
        return Err("contract review cannot accept while listing issues".into());
    }
    validate_texts("contract review issue", &gate.issues)?;
    validate_texts("contract review strength", &gate.strengths)?;
    if gate.decision == "revise" && gate.issues.is_empty() {
        return Err("contract review must name issues when requesting revision".into());
    }
    Ok(())
}

fn validate_verdict(verdict: &TaskVerdict, contract: &TaskContract) -> Result<(), String> {
    if verdict.schema_version != 1 || !matches!(verdict.decision.as_str(), "accept" | "revise") {
        return Err("acceptance verdict has an invalid schemaVersion or decision".into());
    }
    require_text("acceptance verdict summary", &verdict.summary)?;
    if verdict.criteria.len() != contract.acceptance_criteria.len() {
        return Err("acceptance verdict must contain exactly one row per criterion".into());
    }
    for criterion in &verdict.criteria {
        validate_id(&criterion.id)?;
        validate_texts("criterion evidence", &criterion.evidence)?;
        if criterion.passed {
            if criterion.evidence.is_empty() || !criterion.gap.trim().is_empty() {
                return Err(format!(
                    "passed criterion {} requires evidence and an empty gap",
                    criterion.id
                ));
            }
        } else {
            require_text("failed criterion gap", &criterion.gap)?;
        }
    }
    validate_texts("contradiction", &verdict.contradictions)?;
    validate_texts("required repair", &verdict.required_repairs)?;
    let expected = contract
        .acceptance_criteria
        .iter()
        .map(|criterion| criterion.id.as_str())
        .collect::<BTreeSet<_>>();
    let actual = verdict
        .criteria
        .iter()
        .map(|criterion| criterion.id.as_str())
        .collect::<BTreeSet<_>>();
    if expected != actual {
        return Err("acceptance verdict criterion ids do not match the contract".into());
    }
    if verdict.decision == "accept" && !verdict_accepted(verdict) {
        return Err(
            "acceptance verdict cannot accept with gaps, contradictions, or repairs".into(),
        );
    }
    if verdict.decision == "revise" && verdict.required_repairs.is_empty() {
        return Err("a revise verdict must name at least one required repair".into());
    }
    Ok(())
}

fn verdict_accepted(verdict: &TaskVerdict) -> bool {
    verdict.decision == "accept"
        && verdict.criteria.iter().all(|criterion| {
            criterion.passed && !criterion.evidence.is_empty() && criterion.gap.trim().is_empty()
        })
        && verdict.contradictions.is_empty()
        && verdict.required_repairs.is_empty()
}

fn validate_id(value: &str) -> Result<(), String> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err("criterion id cannot be empty".into());
    };
    if !first.is_ascii_lowercase()
        || chars.any(|character| {
            !(character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
        })
    {
        return Err(format!("invalid criterion id: {value}"));
    }
    Ok(())
}

fn require_text(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{label} cannot be empty"))
    } else {
        Ok(())
    }
}

fn validate_texts(label: &str, values: &[String]) -> Result<(), String> {
    for value in values {
        require_text(label, value)?;
    }
    Ok(())
}
fn contract_prompt(objective: &str, preference_json: &str) -> String {
    format!(
        r#"You are the contract-distillation stage of an autonomous task run.
The rough objective is an intent seed, not permission to implement a generic interpretation.
Investigate the repository, its current documentation and configuration, durable memory, and relevant recorded sessions with read-only tools.
Do not modify files, execute commands, ask the user, or begin implementation.
Infer stable preferences only from repeated accepted/rejected evidence; repository facts override remembered claims.
Turn the objective into concrete observable outcomes without adding unrelated scope, retries, telemetry, abstractions, or speculative features.
Return exactly one JSON object and no prose, matching autonomy-contract-v1.schema.json.
Every startingState and sourcesOfTruth entry needs an exact source path or recorded source and one fact.
Every acceptance criterion needs a stable lowercase id, a user-visible outcome, and the exact observation that disproves or confirms it.
Every evidence item must reference one criterion and use one kind from source, configuration, manifest, history, product-output, artifact.
The implementation plan is ordered but contains no placeholder work.
Preserve roughObjective byte-for-byte.

Rough objective:
{objective}

Masked preference evidence gathered before this stage:
{preference_json}"#
    )
}

fn contract_review_prompt(objective: &str, contract_json: &str, preference_json: &str) -> String {
    format!(
        r#"Act as an independent contract critic with no prior conversation.
Use read-only tools to challenge the candidate against the repository and the supplied masked preference evidence.
Reject a contract that merely restates the rough objective, invents scope, uses implementation details as outcomes, lacks disprovable observations, omits a material boundary, or relies on a claim without a source.
Do not modify files, execute commands, ask the user, or implement the task.
Return exactly one JSON object and no prose with keys: schemaVersion=1, decision (accept or revise), summary, issues (array), strengths (array).
Accept only when issues is empty.

Rough objective:
{objective}

Candidate contract:
{contract_json}

Preference evidence:
{preference_json}"#
    )
}

fn contract_revision_prompt(objective: &str, contract_json: &str, gate: &ContractGate) -> String {
    let gate_json = serde_json::to_string_pretty(gate).unwrap_or_else(|_| "{}".into());
    format!(
        r#"Revise the candidate task contract to resolve every critic issue without expanding the rough objective.
Re-read sources when an issue challenges a fact.
Return exactly one complete JSON object matching autonomy-contract-v1.schema.json and no prose.
Preserve roughObjective byte-for-byte.

Rough objective:
{objective}

Candidate contract:
{contract_json}

Critic verdict:
{gate_json}"#
    )
}

fn execution_prompt(contract_json: &str, run_directory: &Path) -> String {
    format!(
        r#"Execute the approved task contract below autonomously and completely.
The contract, not the rough phrase that produced it, is the finish line.
Use repository conventions and sources of truth; do not ask the user or invent scope.
Implement every planned change, remove superseded paths, and gather exactly the required evidence.
Do not claim success from your own implementation narrative: an independent read-only reviewer will inspect the resulting state.
Do not stop at a plan, scaffold, partial subset, or list of future work.
Write no autonomy protocol files yourself; the harness owns the run directory at {}.
If an action is irreversible outside the repository or costs money, leave that criterion visibly unmet instead of performing it.
Your final response is a concise execution record naming changed artifacts and observed evidence.

Approved contract:
{}"#,
        run_directory.display(),
        contract_json
    )
}

fn acceptance_review_prompt(contract_json: &str, execution_text: &str) -> String {
    format!(
        r#"Act as the independent acceptance reviewer for an autonomous task.
Inspect the current repository and durable product output with read-only tools.
Do not trust the executor's summary, modify files, execute commands, ask the user, or broaden the contract.
Evaluate only the approved acceptance criteria and rejection conditions.
Return exactly one JSON object matching autonomy-verdict-v1.schema.json and no prose.
Include exactly one criteria row for each contract id.
A criterion passes only with exact source, configuration, manifest, history, product-output, or artifact evidence; an implementation claim is not evidence.
Set decision=accept only when every criterion passed with non-empty evidence, every gap is empty, and contradictions and requiredRepairs are empty.
Otherwise set decision=revise and name the smallest concrete repairs.

Approved contract:
{contract_json}

Executor's untrusted summary:
{execution_text}"#
    )
}

fn repair_prompt(contract_json: &str, verdict: &TaskVerdict) -> String {
    let verdict_json = serde_json::to_string_pretty(verdict).unwrap_or_else(|_| "{}".into());
    format!(
        r#"The independent reviewer rejected the current result.
Repair every required item below, keeping the approved contract and its non-goals unchanged.
Inspect the actual state before editing, remove any superseded implementation, and gather the required observable evidence.
Do not ask the user, replace the requested outcome with an easier one, or stop after describing the gap.
Return a concise execution record when the repairs are complete.

Approved contract:
{contract_json}

Reviewer verdict:
{verdict_json}"#
    )
}

fn collect_preference_evidence(
    cwd: &Path,
    objective: &str,
    allow_command: bool,
) -> PreferenceEvidence {
    let mut evidence = PreferenceEvidence::default();
    for path in preference_profile_paths(cwd) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if !text.trim().is_empty() {
            evidence.local_profiles.push(EvidenceFact {
                source: path.display().to_string(),
                fact: truncate(&text, 8_000),
            });
        }
    }

    if !allow_command {
        evidence.unavailable_reason =
            Some("Transcript Lake search requires --allow-command or --yolo".into());
        return evidence;
    }

    let binary = env::var("TRANSCRIPT_LAKE_BIN").unwrap_or_else(|_| "transcript-lake".into());
    let queries = preference_queries(objective);
    let mut seen = BTreeSet::new();
    let mut any_search_read = false;
    for query in queries {
        let output = match Command::new(&binary)
            .args(["search", &query, "--limit", "12", "--json"])
            .output()
        {
            Ok(output) if output.status.success() => output,
            Ok(_) => continue,
            Err(_) => continue,
        };
        let Ok(rows) = serde_json::from_slice::<Vec<Value>>(&output.stdout) else {
            continue;
        };
        any_search_read = true;
        for row in rows {
            let event_type = row
                .get("event_type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !matches!(event_type, "user" | "assistant") {
                continue;
            }
            let text = row.get("text").and_then(Value::as_str).unwrap_or_default();
            if text.trim().is_empty() {
                continue;
            }
            let timestamp = row
                .get("ts")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let session_id = row
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let key = format!("{timestamp}\0{session_id}\0{event_type}\0{text}");
            if !seen.insert(key) {
                continue;
            }
            evidence.transcript_matches.push(PreferenceMatch {
                query: query.clone(),
                timestamp,
                runtime: row
                    .get("runtime")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                session_id,
                event_type: event_type.to_string(),
                text: truncate(text, PREFERENCE_TEXT_LIMIT),
            });
            if evidence.transcript_matches.len() >= PREFERENCE_MATCH_LIMIT {
                break;
            }
        }
        if evidence.transcript_matches.len() >= PREFERENCE_MATCH_LIMIT {
            break;
        }
    }
    if !any_search_read {
        evidence.unavailable_reason = Some(format!(
            "{} was unavailable or returned no successful search",
            binary
        ));
    }
    evidence
}

fn preference_profile_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![cwd.join(".jeden/autonomy-preferences.md")];
    if let Ok(home) = env::var("HOME") {
        paths.push(PathBuf::from(home).join(".jeden/autonomy-preferences.md"));
    }
    paths
}

fn preference_queries(objective: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "about", "after", "before", "build", "change", "create", "dla", "from", "into", "jest",
        "make", "oraz", "prosze", "przez", "task", "that", "this", "with", "zeby", "zrob",
        "zrobic",
    ];
    let mut unique = BTreeSet::new();
    let mut queries = Vec::new();
    for word in objective.split(|character: char| !character.is_alphanumeric()) {
        let normalized = word.to_lowercase();
        if normalized.chars().count() < 4 || STOP.contains(&normalized.as_str()) {
            continue;
        }
        if unique.insert(normalized.clone()) {
            queries.push(normalized);
        }
        if queries.len() == 6 {
            break;
        }
    }
    if queries.is_empty() {
        queries.push(truncate(objective, 80));
    }
    queries
}

fn run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    format!("{millis}-{suffix}")
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())? + "\n";
    write_text(path, &text)
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("artifact path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}
