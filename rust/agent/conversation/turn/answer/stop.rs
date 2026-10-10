//! Keep the Stop refusal when persisting its diagnostic also fails.
use super::*;

/// The rule an answer refused by a `Stop` hook is recorded under.
const STOP_RULE: &str = "stop-hook";

impl Conversation {
    /// The last gate before an answer is shown: the `Stop` hooks read the exact
    /// text the operator would receive. A refusal is never shown as the answer;
    /// it goes back to the model as the next instruction and the turn goes on,
    /// so an answer that a hook rejects (open work, a missing blocker id) is
    /// replaced by more work instead of reaching the operator first and being
    /// refused after. `max_steps`, when the operator set one, still bounds it.
    pub(in super::super) fn answer_refused(
        &mut self,
        args: &Args,
        step: u32,
        text: &str,
        prepared: &mut Prepared,
        hooks: &RunHooks,
    ) -> Result<bool, String> {
        let session = self.recorder.path();
        let name = session.file_name().ok_or_else(|| {
            format!(
                "cannot run Stop hook: session path {} has no session name",
                session.display()
            )
        })?;
        let session_id = name.to_str().ok_or_else(|| {
            format!(
                "cannot run Stop hook: session name {name:?} in {} is not UTF-8",
                session.display()
            )
        })?;
        let Some(reason) = crate::hooks::answer_stop_block(
            &args.cwd,
            session_id,
            &session.join("transcript.jsonl"),
            text,
            prepared.stop_refused,
            args.allow_command,
        ) else {
            return Ok(false);
        };
        let failed = |operation: &str, error: String| {
            format!("answer refused by Stop hook: {reason}; {operation} failed: {error}")
        };
        prepared.stop_refused = true;
        let completion_state = if prepared.tracks_completion {
            Some(
                crate::completion::read_state(&session)
                    .map_err(|error| failed("reading completion state", error))?,
            )
        } else {
            None
        };
        self.recorder
            .record(
                task_contract::VIOLATION_EVENT,
                json!({
                    "step": step,
                    "rule": STOP_RULE,
                    "outcome": "requested",
                    "message": &reason,
                    "prompt": &reason,
                    "completion": completion_state.as_ref().map(crate::completion::snapshot_value),
                }),
            )
            .map_err(|error| failed("recording stop-hook violation", error))?;
        if let Some(state) = completion_state.as_ref() {
            self.publish_completion(state, hooks)
                .map_err(|error| failed("publishing completion state", error))?;
            let open_requests = crate::completion::open_asks(state, &session)
                .map_err(|error| failed("reading open completion requests", error))?;
            self.recorder.record(
                "completion_stop_refusal",
                json!({
                    "step": step,
                    "reason": &reason,
                    "status": state.status(),
                    "openTasks": state.tasks.iter().filter(|task| !task.status.terminal()).collect::<Vec<_>>(),
                    "openRequests": open_requests,
                }),
            ).map_err(|error| failed("recording completion_stop_refusal", error))?;
        }
        hooks.note("answer refused by a Stop hook; continuing the work");
        self.messages
            .push(json!({ "role": "user", "content": reason }));
        Ok(true)
    }
}
