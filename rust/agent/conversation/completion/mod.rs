use super::super::*;
use crate::completion::{self, CompletionReview, CompletionState, IntakePlan};

const INTAKE: &str = include_str!("../../../completion/prompts/intake.txt");
const REVIEW: &str = include_str!("../../../completion/prompts/review.txt");
const CONTEXT_PREFIX: &str = "[Jeden completion authority]";

impl Conversation {
    pub(super) fn tracks_completion(&self, args: &Args) -> bool {
        self.manages_completion && !args.model_only
    }

    pub(super) fn publish_completion(&mut self, state: &CompletionState, hooks: &RunHooks<'_>) -> Result<(), String> {
        let mut value = completion::snapshot_value(state);
        value["sessionPath"] = json!(self.recorder.path());
        self.recorder.record("completion_state", value.clone())?;
        hooks.trace(&TraceEvent::CompletionState { state: &value });
        Ok(())
    }

    pub(super) fn completion_failure(&mut self, operation: &str, error: &str, hooks: &RunHooks<'_>) -> String {
        match completion::observed_blocker(&self.recorder.path(), operation, error) {
            Ok(state) => {
                if let Err(record_error) = self.publish_completion(&state, hooks) {
                    return format!("{error}; recording completion state failed: {record_error}");
                }
            }
            Err(record_error) => return format!("{error}; persisting unfinished work failed: {record_error}"),
        }
        format!("Work remains open ({operation}): {error}. Session: {}", self.recorder.path().display())
    }

    pub(super) fn capture_completion(&mut self, args: &Args, task: &str, hooks: &RunHooks<'_>) -> Result<String, String> {
        completion::migrate_workspace(&args.cwd, Some(&self.recorder.path()))?;
        let (id, state) = completion::capture_request(&self.recorder.path(), &args.cwd, task)?;
        crate::agent::update_last_session_path(&args.cwd, &self.recorder.path())?;
        self.publish_completion(&state, hooks)?;
        Ok(id)
    }

    pub(super) fn prepare_completion(&mut self, args: &Args, hooks: &RunHooks<'_>) -> Result<(), String> {
        loop {
            let state = completion::read_state(&self.recorder.path())?;
            let Some(request) = state.requests.iter().find(|request| !request.planned && !request.paused) else {
                break;
            };
            let input = json!({
                "request": request,
                "retainedTasks": state.tasks,
                "workspace": args.cwd,
                "executionGrants": {"write": args.allow_write, "command": args.allow_command},
            });
            hooks.note("recording acceptance requirements before execution");
            let (text, inspector) = self.inspect_completion(args, INTAKE, &input, hooks)
                .map_err(|error| self.completion_failure("task_intake", &error, hooks))?;
            let plan: IntakePlan = serde_json::from_str(crate::protocol::extract_json_object(&text)?)
                .map_err(|error| self.completion_failure("task_intake", &format!("invalid task intake: {error}"), hooks))?;
            let state = completion::plan_request(&self.recorder.path(), state.revision, &request.id, plan)
                .map_err(|error| self.completion_failure("task_intake", &error, hooks))?;
            self.recorder.record("completion_review", json!({
                "stage": "intake", "reviewerSession": inspector, "revision": state.revision,
            }))?;
            self.publish_completion(&state, hooks)?;
        }
        if self.reconcile_completion {
            self.reconcile_completion = false;
            hooks.note("inspecting retained results before continuing interrupted work");
            let prior = self.messages.iter().rev()
                .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
                .and_then(|message| message.get("content").and_then(Value::as_str))
                .unwrap_or_default().to_string();
            let _ = self.verify_completion(args, &prior, Value::Null, hooks)?;
        }
        Ok(())
    }

    pub(super) fn refresh_completion_context(&mut self, hooks: &RunHooks<'_>) -> Result<(), String> {
        let state = completion::read_state(&self.recorder.path())?;
        if !state.actionable() && !state.complete()
            && matches!(state.status(), "paused" | "blocked") {
            self.publish_completion(&state, hooks)?;
            return Err(format!("Work remains {}. Session: {}", state.status(), self.recorder.path().display()));
        }
        let content = format!("{CONTEXT_PREFIX}\n{}", completion::model_context(&state));
        if let Some(message) = self.messages.iter_mut().find(|message| {
            message.get("role").and_then(Value::as_str) == Some("system")
                && message.get("content").and_then(Value::as_str)
                    .is_some_and(|content| content.starts_with(CONTEXT_PREFIX))
        }) {
            message["content"] = json!(content);
        } else {
            self.messages.insert(usize::from(!self.messages.is_empty()), json!({"role": "system", "content": content}));
        }
        Ok(())
    }

    pub(super) fn verify_completion(
        &mut self,
        args: &Args,
        answer: &str,
        report: Value,
        hooks: &RunHooks<'_>,
    ) -> Result<bool, String> {
        let state = completion::read_state(&self.recorder.path())?;
        if state.complete() {
            return Ok(true);
        }
        let input = json!({
            "sourceSession": self.recorder.path(),
            "state": completion::snapshot_value(&state),
            "executionEvidence": completion::review_evidence(&self.recorder.path())?,
            "proposedAnswer": answer,
            "deliveryReport": report,
            "executionGrants": {"write": args.allow_write, "command": args.allow_command},
        });
        hooks.note("independently checking all retained acceptance requirements");
        let (text, inspector) = self.inspect_completion(args, REVIEW, &input, hooks)
            .map_err(|error| self.completion_failure("acceptance_review", &error, hooks))?;
        let review: CompletionReview = serde_json::from_str(crate::protocol::extract_json_object(&text)?)
            .map_err(|error| self.completion_failure("acceptance_review", &format!("invalid acceptance review: {error}"), hooks))?;
        let reviewed = match completion::apply_review(&self.recorder.path(), &inspector, state.revision, review) {
            Ok(state) => state,
            Err(error) => {
                self.recorder.record("completion_rejected", json!({
                    "stage": "verification", "reason": error, "reviewerSession": inspector,
                }))?;
                self.messages.push(json!({"role": "user", "content": format!(
                    "Jeden did not accept completion: {error}. Keep the work open. Do not repeat completed effects. \
                     Inspect and supply the actual missing evidence or perform the unfinished work."
                )}));
                return Ok(false);
            }
        };
        self.recorder.record("completion_review", json!({
            "stage": "acceptance", "reviewerSession": inspector,
            "revision": reviewed.revision, "accepted": reviewed.complete(),
        }))?;
        self.publish_completion(&reviewed, hooks)?;
        if reviewed.complete() {
            return Ok(true);
        }
        self.recorder.record("completion_rejected", json!({
            "stage": "final", "reason": "retained acceptance requirements remain unfinished",
            "state": completion::snapshot_value(&reviewed),
        }))?;
        if !reviewed.actionable() && matches!(reviewed.status(), "blocked" | "paused") {
            return Err(format!("Work remains {}. {} Session: {}", reviewed.status(),
                reviewed.tasks.iter().filter_map(|task| task.reason.as_deref()).collect::<Vec<_>>().join("; "),
                self.recorder.path().display()));
        }
        self.messages.push(json!({"role": "user", "content": format!(
            "Jeden withheld the final answer because work remains. Continue the concrete missing work below, \
             not a promise or a next-steps list. Preserve already verified results.\n{}",
            completion::model_context(&reviewed)
        )}));
        Ok(false)
    }

    fn inspect_completion(
        &self,
        args: &Args,
        instruction: &str,
        input: &Value,
        hooks: &RunHooks<'_>,
    ) -> Result<(String, PathBuf), String> {
        let mut inspector = Conversation::new_inspection(&args.cwd)?;
        inspector.recorder.record("agent_state", json!({
            "purpose": "completion_inspection", "sourceSession": self.recorder.path(),
            "allowWrite": false, "allowCommand": false,
        }))?;
        let mut read_args = args.clone();
        read_args.allow_write = false;
        read_args.allow_command = false;
        read_args.yolo = false;
        read_args.model_only = false;
        read_args.autonomous = true;
        read_args.goal = None;
        let mut read_hooks = RunHooks {
            cancel: hooks.cancel.clone(),
            interactive: false,
            progress: Box::new(|message| hooks.note(message)),
            stream: Box::new(|_| {}),
            trace: Box::new(|event| hooks.trace(event)),
            ask_user: None,
            approve: Box::new(|_, _| false),
            goal_event: None,
        };
        let prompt = format!("{instruction}\n\nNative controller input:\n{input}");
        let text = inspector.run_turn(&read_args, &prompt, &[], &mut read_hooks)?;
        Ok((text, inspector.session_path()))
    }

    pub(crate) fn completion_state(&self) -> Result<Value, String> {
        completion::snapshot(&self.recorder.path())
    }

    pub(crate) fn continue_work(&mut self, args: &Args, hooks: &mut RunHooks<'_>) -> Result<String, String> {
        let state = completion::read_state(&self.recorder.path())?;
        if state.complete() {
            return Ok("No retained work remains.".into());
        }
        if state.status() == "paused" {
            return Err("Retained work is paused; resume its tasks before continuing.".into());
        }
        self.continuation = true;
        self.reconcile_completion = true;
        self.run_turn(args, "Continue every retained unfinished task. Inspect prior effects before retrying anything.", &[], hooks)
    }
}
