use crate::agent::{Conversation, RunHooks};
use crate::Args;
use pursuit::{
    collect_preference_evidence, pursue, PursuitConfig, Stage, StageResponse, StageRunner,
};
use serde_json::json;
use std::env;
use std::path::PathBuf;

struct JedenStageRunner {
    read_only_args: Args,
    execution_args: Args,
    hooks: RunHooks<'static>,
    planner: Conversation,
    executor: Option<Conversation>,
    contract_reviewer: Option<(usize, Conversation)>,
    acceptance_reviewer: Option<(usize, Conversation)>,
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
            planner: Conversation::new(&args.cwd)?,
            executor: None,
            contract_reviewer: None,
            acceptance_reviewer: None,
        })
    }

    fn run_planner(&mut self, prompt: &str) -> Result<StageResponse, String> {
        let text = self.planner.run_turn(
            &self.read_only_args,
            prompt,
            &[],
            &mut self.hooks,
        )?;
        Ok(StageResponse::new(text, Some(self.planner.session_path())))
    }

    fn run_executor(&mut self, prompt: &str) -> Result<StageResponse, String> {
        if self.executor.is_none() {
            self.executor = Some(Conversation::new(&self.execution_args.cwd)?);
        }
        let executor = self.executor.as_mut().expect("executor was initialized");
        let text = executor.run_turn(
            &self.execution_args,
            prompt,
            &[],
            &mut self.hooks,
        )?;
        Ok(StageResponse::new(text, Some(executor.session_path())))
    }

    fn run_contract_reviewer(
        &mut self,
        round: usize,
        prompt: &str,
    ) -> Result<StageResponse, String> {
        if self.contract_reviewer.as_ref().map(|(active, _)| *active) != Some(round) {
            self.contract_reviewer = Some((round, Conversation::new(&self.read_only_args.cwd)?));
        }
        let (_, reviewer) = self
            .contract_reviewer
            .as_mut()
            .expect("contract reviewer was initialized");
        let text = reviewer.run_turn(
            &self.read_only_args,
            prompt,
            &[],
            &mut self.hooks,
        )?;
        Ok(StageResponse::new(text, Some(reviewer.session_path())))
    }

    fn run_acceptance_reviewer(
        &mut self,
        round: usize,
        prompt: &str,
    ) -> Result<StageResponse, String> {
        if self.acceptance_reviewer.as_ref().map(|(active, _)| *active) != Some(round) {
            self.acceptance_reviewer = Some((round, Conversation::new(&self.read_only_args.cwd)?));
        }
        let (_, reviewer) = self
            .acceptance_reviewer
            .as_mut()
            .expect("acceptance reviewer was initialized");
        let text = reviewer.run_turn(
            &self.read_only_args,
            prompt,
            &[],
            &mut self.hooks,
        )?;
        Ok(StageResponse::new(text, Some(reviewer.session_path())))
    }
}

impl StageRunner for JedenStageRunner {
    fn run(&mut self, stage: &Stage, prompt: &str) -> Result<StageResponse, String> {
        match stage {
            Stage::Distill | Stage::ContractRevision { .. } => self.run_planner(prompt),
            Stage::ContractReview { round } => self.run_contract_reviewer(*round, prompt),
            Stage::Execute { .. } | Stage::Repair { .. } => self.run_executor(prompt),
            Stage::AcceptanceReview { round } => {
                self.run_acceptance_reviewer(*round, prompt)
            }
        }
    }
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
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
    let preference_evidence = collect_preference_evidence(
        &args.cwd,
        &objective,
        transcript_lake.as_deref(),
    );
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
