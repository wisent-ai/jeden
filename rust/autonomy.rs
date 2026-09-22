use crate::agent::{Conversation, RunHooks};
use crate::Args;
use pursuit::{
    collect_preference_evidence, pursue, PursuitConfig, Stage, StageResponse, StageRunner,
};
use serde_json::json;
use std::env;
use std::path::PathBuf;

pub(crate) mod requests;

struct JedenStageRunner {
    read_only_args: Args,
    execution_args: Args,
    hooks: RunHooks<'static>,
    conversations: StageConversations,
}

impl JedenStageRunner {
    fn new(args: &Args) -> Result<Self, String> {
        let mut read_only_args = args.clone();
        read_only_args.command = "run".into();
        read_only_args.positionals.clear();
        read_only_args.allow_write = false;
        read_only_args.allow_command = false;
        read_only_args.yolo = false;
        read_only_args.goal = None;
        read_only_args.autonomous = true;

        let mut execution_args = args.clone();
        execution_args.command = "run".into();
        execution_args.positionals.clear();
        execution_args.goal = None;
        execution_args.autonomous = true;

        Ok(Self {
            read_only_args,
            execution_args,
            hooks: RunHooks::inert(),
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

impl StageRunner for JedenStageRunner {
    fn run(&mut self, stage: &Stage, prompt: &str) -> Result<StageResponse, String> {
        let args = if stage.read_only() {
            &self.read_only_args
        } else {
            &self.execution_args
        };
        let conversation = self.conversations.get(stage, &args.cwd)?;
        let text = conversation.run_turn(args, prompt, &[], &mut self.hooks)?;
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
                if self.executor.is_none() {
                    self.executor = Some(Conversation::new_stage(cwd)?);
                }
                Ok(self.executor.as_mut().expect("executor initialized"))
            }
            Stage::ContractReview { round } | Stage::AcceptanceReview { round } => {
                let slot = if matches!(stage, Stage::ContractReview { .. }) {
                    &mut self.contract_reviewer
                } else {
                    &mut self.acceptance_reviewer
                };
                if slot.as_ref().map(|(active, _)| active) != Some(round) {
                    *slot = Some((*round, Conversation::new_stage(cwd)?));
                }
                Ok(&mut slot.as_mut().expect("reviewer initialized").1)
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

    let transcript_lake = if args.allow_command || args.yolo {
        Some(PathBuf::from(
            env::var("TRANSCRIPT_LAKE_BIN").unwrap_or_else(|_| "transcript-lake".into()),
        ))
    } else {
        None
    };
    let preference_evidence =
        collect_preference_evidence(&args.cwd, &objective, transcript_lake.as_deref());
    let config = PursuitConfig::new(&args.cwd, objective, preference_evidence);
    let mut runner = JedenStageRunner::new(args)?;
    let outcome = pursue(config, &mut runner).map_err(|error| error.to_string())?;

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
