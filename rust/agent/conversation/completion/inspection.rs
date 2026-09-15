//! The read-only inspection every completion stage asks, and the one
//! correction it is given before the stage records a durable blocker.
//!
//! Intake and acceptance review both read a JSON answer from a fresh
//! inspection conversation, and both used to turn one unusable answer — a
//! truncated object, a missing field — into a `blocker` that ended the turn
//! with the user's request retained and unfinished. The refusal is the
//! inspection's own to repair, so it is quoted back to a new inspection once.
//! The second refusal is recorded and stops the turn: a correction, never a
//! retry loop.
//!
//! One refusal cannot be repaired by wording alone. An answer the output
//! budget cut mid-JSON was already longer than the budget allows, and the
//! correction adds the refusal to what it must say, so that single correction
//! is asked with `INSPECTION_RETRY_OUTPUT_TOKENS` and with the shortest form
//! of the answer named in the instruction.

use super::super::*;

impl Conversation {
    pub(super) fn inspect_completion(
        &self,
        args: &Args,
        instruction: &str,
        input: &Value,
        hooks: &RunHooks<'_>,
        budget: u32,
    ) -> Result<(String, PathBuf), String> {
        let mut inspector = Conversation::new_inspection(&args.cwd)?;
        inspector.recorder.record(
            "agent_state",
            json!({
                "purpose": "completion_inspection", "sourceSession": self.recorder.path(),
                "allowWrite": false, "allowCommand": false,
            }),
        )?;
        let mut read_args = args.clone();
        read_args.allow_write = false;
        read_args.allow_command = false;
        read_args.yolo = false;
        read_args.model_only = false;
        read_args.autonomous = true;
        read_args.goal = None;
        read_args.max_tokens = Some(read_args.max_tokens.unwrap_or(budget).max(budget));
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

    /// Ask one inspection for a usable answer, correcting a single refusal.
    ///
    /// `accept` reads the answer and either returns the stage's value or the
    /// exact refusal to quote back. Every refusal is recorded as
    /// `completion_rejected` against the inspection that produced it, so the
    /// session retains what was refused and why even when the correction
    /// succeeds.
    pub(super) fn corrected_inspection<T>(
        &mut self,
        args: &Args,
        stage: &str,
        instruction: &str,
        input: &Value,
        hooks: &RunHooks<'_>,
        accept: &mut dyn FnMut(&str) -> Result<T, String>,
    ) -> Result<(T, PathBuf), String> {
        let mut correction: Option<String> = None;
        loop {
            let asked = match &correction {
                None => instruction.to_string(),
                Some(refusal) if cut_off(refusal) => format!(
                    "{instruction}\n\nThe previous answer was refused: {refusal}\n\
                     Return only the corrected JSON object, complete and closed, \
                     and keep every explanation to one sentence so it fits."
                ),
                Some(refusal) => format!(
                    "{instruction}\n\nThe previous answer was refused: {refusal}\n\
                     Return only the corrected JSON object, complete and closed."
                ),
            };
            let budget = match &correction {
                Some(refusal) if cut_off(refusal) => {
                    crate::completion::INSPECTION_RETRY_OUTPUT_TOKENS
                }
                _ => crate::completion::INSPECTION_OUTPUT_TOKENS,
            };
            let (text, inspector) = self
                .inspect_completion(args, &asked, input, hooks, budget)
                .map_err(|error| self.completion_failure(stage, &error, hooks))?;
            let refusal = match accept(&text) {
                Ok(value) => return Ok((value, inspector)),
                Err(refusal) => refusal,
            };
            self.recorder.record(
                "completion_rejected",
                json!({
                    "stage": stage, "reason": refusal, "reviewerSession": inspector,
                }),
            )?;
            if correction.is_some() {
                return Err(self.completion_failure(stage, &refusal, hooks));
            }
            correction = Some(refusal);
        }
    }
}

/// True for the one refusal that more words cannot repair: the answer ran past
/// the output budget and stopped mid-JSON.
fn cut_off(refusal: &str) -> bool {
    refusal.contains(crate::protocol::INCOMPLETE_ANSWER)
}

#[cfg(test)]
#[path = "inspection_tests.rs"]
mod tests;
