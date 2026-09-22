pub(crate) mod budget;
mod evidence;
mod runner;
mod store;
use crate::Args;
use pursuit::{collect_preference_evidence, pursue, PreferenceEvidence, PursuitConfig, RunReceipt};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use store::Store;

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub(crate) enum Mode {
    Submit(PathBuf),
    Status(String),
    Resume(String),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    request_id: String,
    initiative_id: String,
    objective: String,
    cwd: PathBuf,
    evidence_refs: Vec<String>,
    budget_usd: String,
    allow_write: bool,
    allow_command: bool,
    #[serde(default)]
    repositories: Vec<String>,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Running,
    Succeeded,
    Failed,
    Blocked,
    Indeterminate,
}
#[derive(Serialize, Deserialize)]
struct Response {
    schema_version: u32,
    request_id: String,
    initiative_id: String,
    state: State,
    run_id: Option<String>,
    run_directory: PathBuf,
    contract: Option<PathBuf>,
    verdict: Option<PathBuf>,
    receipt: Option<PathBuf>,
    source_revision: Option<String>,
    source_revisions: BTreeMap<String, String>,
    spent_usd: Option<String>,
    evidence_refs: Vec<String>,
    error: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct ExecutionOptions {
    model: Option<String>,
    max_tokens: Option<u32>,
    max_steps: Option<u32>,
}

fn emit(response: &Response) -> Result<String, String> {
    serde_json::to_string_pretty(response)
        .map(|v| v + "\n")
        .map_err(|e| e.to_string())
}

pub(super) fn command(args: &Args, mode: &Mode) -> Result<String, String> {
    let (store, request, resume) = match mode {
        Mode::Status(id) => return emit(&Store::existing(id)?.response()?),
        Mode::Resume(id) => {
            let store = Store::existing(id)?;
            let request = store
                .get::<Request>("request")?
                .ok_or("stored request payload is missing")?;
            (store, request, true)
        }
        Mode::Submit(path) => {
            let request: Request = store::read_json(path)?;
            validate(&request)?;
            if (request.allow_write && !args.allow_write)
                || (request.allow_command && !args.allow_command)
            {
                return Err("authority_required: request-file fields cannot grant write or command access; pass the matching invocation grants".into());
            }
            let store = Store::open(&request)?;
            (store, request, false)
        }
    };
    validate(&request)?;
    let Some(_claim) = store.claim()? else {
        return emit(&store.response()?);
    };
    let mut response = store.get::<Response>("response")?.unwrap_or(Response {
        schema_version: SCHEMA_VERSION,
        request_id: request.request_id.clone(),
        initiative_id: request.initiative_id.clone(),
        state: State::Running,
        run_id: None,
        run_directory: store.directory.join("runs"),
        contract: None,
        verdict: None,
        receipt: None,
        source_revision: None,
        source_revisions: BTreeMap::new(),
        spent_usd: None,
        evidence_refs: request.evidence_refs.clone(),
        error: None,
    });
    if response.state == State::Succeeded
        || response.state == State::Failed
        || response.state == State::Indeterminate
        || (response.state == State::Blocked && !resume)
    {
        return emit(&response);
    }
    let execution = match store.get::<ExecutionOptions>("execution_options")? {
        Some(options) => options,
        None => {
            let options = ExecutionOptions {
                model: args.model.clone(),
                max_tokens: args.max_tokens,
                max_steps: args.max_steps,
            };
            store.set("execution_options", &options)?;
            options
        }
    };
    let mut invocation = args.clone();
    invocation.cwd = request.cwd.clone();
    invocation.cwd_explicit = true;
    invocation.allow_write = request.allow_write;
    invocation.allow_command = request.allow_command;
    invocation.yolo = false;
    invocation.model = execution.model;
    invocation.max_tokens = execution.max_tokens;
    invocation.max_steps = execution.max_steps;
    invocation.pursuit_request = None;
    invocation.positionals = vec![request.objective.clone()];
    invocation.autonomous = true;
    response.state = State::Running;
    response.error = None;
    store.set("response", &response)?;
    let result = execute(&store, &request, &invocation, &mut response);
    if let Err(error) = result {
        if !matches!(response.state, State::Indeterminate | State::Failed) {
            response.state = State::Blocked;
        }
        response.error = Some(error);
    }
    response.spent_usd = budget::spent(&store.directory)?;
    store.set("response", &response)?;
    emit(&response)
}

fn validate(request: &Request) -> Result<(), String> {
    if request.schema_version != SCHEMA_VERSION
        || !store::identifier(&request.request_id)
        || !store::identifier(&request.initiative_id)
        || request.objective.trim().is_empty()
        || request.evidence_refs.iter().any(|v| v.trim().is_empty())
    {
        return Err(
            "invalid durable pursuit request schema, identity, objective or evidence reference"
                .into(),
        );
    }
    if !request.cwd.is_absolute()
        || request
            .cwd
            .canonicalize()
            .map_err(|e| format!("request workspace: {e}"))?
            != request.cwd
    {
        return Err("request cwd must be an absolute canonical checkout path".into());
    }
    let budget = request
        .budget_usd
        .parse::<rust_decimal::Decimal>()
        .map_err(|e| format!("request budget: {e}"))?;
    if budget <= rust_decimal::Decimal::ZERO {
        return Err("request budget must be positive".into());
    }
    Ok(())
}

fn execute(
    store: &Store,
    request: &Request,
    args: &Args,
    response: &mut Response,
) -> Result<(), String> {
    if store
        .get::<BTreeMap<String, String>>("starting_revisions")?
        .is_none()
    {
        let starting = evidence::snapshot(request, true)?;
        store.set("starting_revisions", &starting)?;
    }
    let preferences = match store.get::<PreferenceEvidence>("preferences")? {
        Some(preferences) => preferences,
        None => {
            let lake = args.allow_command.then(|| {
                PathBuf::from(
                    std::env::var("TRANSCRIPT_LAKE_BIN")
                        .unwrap_or_else(|_| "transcript-lake".into()),
                )
            });
            let preferences =
                collect_preference_evidence(&request.cwd, &request.objective, lake.as_deref());
            store.set("preferences", &preferences)?;
            preferences
        }
    };
    let mut config = PursuitConfig::new(&request.cwd, &request.objective, preferences);
    config.artifact_root = Some(store.directory.join("runs"));
    budget::activate(&store.directory, &request.budget_usd)?;
    let mut runner = runner::DurableRunner::new(store, args)?;
    match pursue(config, &mut runner) {
        Ok(outcome) => {
            response.run_id = Some(outcome.run_id);
            response.run_directory = outcome.run_directory;
            response.contract = Some(outcome.contract_path);
            response.verdict = Some(outcome.verdict_path);
            response
                .evidence_refs
                .push(outcome.receipt_path.display().to_string());
            response.receipt = Some(outcome.receipt_path);
            let reviewed = store
                .get::<BTreeMap<String, String>>("reviewed_revisions")?
                .ok_or("independent reviewer did not bind source revisions")?;
            if evidence::snapshot(request, true)? != reviewed {
                return Err(
                    "source changed after independent acceptance; release is refused".into(),
                );
            }
            if request.allow_write {
                evidence::pushed(request, &reviewed)?;
            }
            response.source_revision = reviewed.get(&evidence::repository(&request.cwd)?).cloned();
            response.source_revisions = reviewed;
            response.state = State::Succeeded;
            Ok(())
        }
        Err(error) => {
            if runner.indeterminate {
                response.state = State::Indeterminate;
            }
            if let Some(path) = &error.receipt_path {
                let receipt: RunReceipt = store::read_json(path)?;
                if matches!(receipt.state.as_str(), "rejected" | "contract_rejected")
                    && !runner.indeterminate
                {
                    response.state = State::Failed;
                }
                response.run_id = Some(receipt.run_id);
                response.run_directory = receipt.run_directory;
                response.contract = receipt.contract_path;
                response.verdict = receipt.verdict_path;
                response.receipt = Some(path.clone());
                response.evidence_refs.push(path.display().to_string());
            }
            Err(error.to_string())
        }
    }
}
