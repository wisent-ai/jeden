use crate::agent::{Conversation, RunHooks};
use crate::Args;
use pursuit::{
    collect_preference_evidence, pursue, ProtocolError, PursuitConfig, RunOutcome, Stage, StageResponse,
    StageRunner,
};
use serde_json::json;
use std::borrow::BorrowMut;
use std::env;
use std::path::PathBuf;

pub(crate) mod requests;

struct JedenStageRunner<H = RunHooks<'static>> {
    read_only_args: Args,
    execution_args: Args,
    hooks: H,
    conversations: StageConversations,
}

impl JedenStageRunner {
    fn new(args: &Args) -> Result<Self, String> {
        Self::controlled(args, RunHooks::inert())
    }
}

impl<H: BorrowMut<RunHooks<'static>>> JedenStageRunner<H> {
    fn controlled(args: &Args, hooks: H) -> Result<Self, String> {
        let mut read_only_args = args.clone();
        read_only_args.command = crate::Command::Run;
        read_only_args.positionals.clear();
        read_only_args.allow_write = false;
        read_only_args.allow_command = false;
        read_only_args.yolo = false;
        read_only_args.goal = None;
        read_only_args.autonomous = true;

        let mut execution_args = args.clone();
        execution_args.command = crate::Command::Run;
        execution_args.positionals.clear();
        execution_args.goal = None;
        execution_args.autonomous = true;

        Ok(Self {
            read_only_args,
            execution_args,
            hooks,
            conversations: StageConversations {
                planner: Conversation::new_stage(&args.cwd)?,
                executor: None,
                contract_reviewer: None,
                acceptance_reviewer: None,
            },
        })
    }

    fn prepare(&mut self, stage: &Stage) -> Result<PathBuf, String> {
        Ok(self
            .conversations
            .get(stage, &self.execution_args.cwd)?
            .session_path())
    }

    fn restore(&mut self, stage: &Stage, path: &std::path::Path) -> Result<(), String> {
        let restored = Conversation::open_stage(&self.execution_args.cwd, path)?;
        *self.conversations.get(stage, &self.execution_args.cwd)? = restored;
        Ok(())
    }
}

impl<H: BorrowMut<RunHooks<'static>>> StageRunner for JedenStageRunner<H> {
    fn run(&mut self, stage: &Stage, prompt: &str) -> Result<StageResponse, String> {
        let hooks = self.hooks.borrow_mut();
        if hooks.cancelled() {
            return Err("pursuit cancelled before the next stage could run".into());
        }
        let args = if stage.read_only() {
            &self.read_only_args
        } else {
            &self.execution_args
        };
        let conversation = self.conversations.get(stage, &args.cwd)?;
        let text = conversation.run_turn(args, prompt, &[], hooks)?;
        Ok(StageResponse::new(text, Some(conversation.session_path())))
    }
}

struct StageConversations {
    planner: Conversation,
    executor: Option<Conversation>,
    contract_reviewer: Option<(usize, Conversation)>,
    acceptance_reviewer: Option<(usize, Conversation)>,
}

impl StageConversations {
    fn get(&mut self, stage: &Stage, cwd: &std::path::Path) -> Result<&mut Conversation, String> {
        match stage {
            Stage::Distill | Stage::ContractRevision { .. } => Ok(&mut self.planner),
            Stage::Execute { .. } | Stage::Repair { .. } => {
                let executor = match self.executor.take() {
                    Some(executor) => executor,
                    None => Conversation::new_stage(cwd)?,
                };
                Ok(self.executor.insert(executor))
            }
            Stage::ContractReview { round } | Stage::AcceptanceReview { round } => {
                let slot = if matches!(stage, Stage::ContractReview { .. }) {
                    &mut self.contract_reviewer
                } else {
                    &mut self.acceptance_reviewer
                };
                // A reviewer serves one round; a later round starts a fresh one.
                let reviewer = match slot.take() {
                    Some((active, reviewer)) if active == *round => reviewer,
                    Some(_) | None => Conversation::new_stage(cwd)?,
                };
                let (_, reviewer) = slot.insert((*round, reviewer));
                Ok(reviewer)
            }
        }
    }
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    if let Some(mode) = &args.pursuit_request {
        return requests::command(args, mode);
    }
    let objective = args.positionals.join(" ");
    if objective.trim().is_empty() {
        return Err("pursue requires a rough objective".into());
    }
    let outcome = pursue_objective(args, objective, &mut RunHooks::inert()).map_err(|error| error.to_string())?;

    if args.json {
        return serde_json::to_string_pretty(&json!({
            "ok": true,
            "runId": outcome.run_id,
            "runDirectory": outcome.run_directory,
            "contract": outcome.contract_path,
            "verdict": outcome.verdict_path,
            "receipt": outcome.receipt_path,
            "summary": outcome.verdict.summary,
        }))
        .map(|text| text + "\n")
        .map_err(|error| error.to_string());
    }

    Ok(format!(
        "{}\ncontract: {}\nverdict: {}\nreceipt: {}\n",
        outcome.verdict.summary,
        outcome.contract_path.display(),
        outcome.verdict_path.display(),
        outcome.receipt_path.display(),
    ))
}

/// Pursue `objective` in `args.cwd` with `args`' grants: the contract, the
/// execution and the independent acceptance Pursuit runs, through Jeden's
/// conversations. `hillclimb` runs one of these per round.
pub(crate) fn pursue_objective(args: &Args, objective: String, hooks: &mut RunHooks<'static>) -> Result<RunOutcome, ProtocolError> {
    let unstarted = |message: String| ProtocolError { message, receipt_path: None };
    if hooks.cancelled() {
        return Err(unstarted("pursuit cancelled before reading preference evidence".into()));
    }
    // Unset means the program on PATH; set to something that is not text is
    // the operator's override unread, refused rather than silently replaced.
    let transcript_lake = if args.allow_command || args.yolo {
        Some(match env::var("TRANSCRIPT_LAKE_BIN") {
            Ok(path) => PathBuf::from(path),
            Err(env::VarError::NotPresent) => PathBuf::from("transcript-lake"),
            Err(error) => return Err(unstarted(format!("TRANSCRIPT_LAKE_BIN cannot be read: {error}"))),
        })
    } else {
        None
    };
    let preference_evidence =
        collect_preference_evidence(&args.cwd, &objective, transcript_lake.as_deref());
    if hooks.cancelled() {
        return Err(unstarted("pursuit cancelled after reading preference evidence".into()));
    }
    let config = PursuitConfig::new(&args.cwd, objective, preference_evidence);
    let mut runner = JedenStageRunner::controlled(args, hooks).map_err(unstarted)?;
    pursue(config, &mut runner)
}
