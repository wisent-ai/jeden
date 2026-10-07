//! What a turn does with an answer: accept a proposed final only after its
//! delivery report and the independent acceptance check, and ask once for a
//! whole answer when the one that arrived is not usable.

use super::super::*;
use super::prompt::Prepared;

/// Corrections one turn asks for when an answer never arrives usable: one,
/// then the turn ends. A correction, never a retry loop - the same bound the
/// completion stages hold for a refused inspection answer.
pub(super) const ANSWER_REPAIRS: u32 = 1;

/// The rule name every unusable-answer correction is recorded under, beside
/// the delivery-report rule of the same event.
const ANSWER_RULE: &str = "model-answer";

/// The rule an answer refused by a `Stop` hook is recorded under.
const STOP_RULE: &str = "stop-hook";

/// The correction one unusable answer is given.
///
/// The correction distinguishes output-budget truncation, incomplete JSON
/// nesting and malformed string escapes. Each needs different advice:
/// shortening a complete provider response cannot repair unclosed brackets.
fn repair_instruction(refusal: &str, cut_off: bool, max_tokens: Option<u32>) -> String {
    let budget = match max_tokens {
        Some(tokens) => format!(" of {tokens} tokens"),
        None => String::new(),
    };
    let advice = if cut_off {
        format!("Your previous answer stopped before it was complete: {refusal}\n\nSend the whole answer again, short enough to finish inside the output budget{budget}.")
    } else if crate::protocol::is_incomplete_answer(refusal) {
        format!("Your previous answer ended with brackets still open: {refusal}\n\nThe provider finished the answer, so it is not too long; an object or array was closed too early or not at all. Check the nesting of every array and object, and close the outer object last.")
    } else {
        format!("Your previous answer could not be read as an action: {refusal}\n\nEvery quote, backslash and newline inside a JSON string must be escaped (\\\", \\\\, \\n); quoting someone's words inside `text` is the usual cause. Rewrite the answer with those escapes rather than resending the same characters.")
    };
    format!(
        "{advice} Answer with exactly one complete JSON object of the action protocol, with nothing before or after it."
    )
}

/// The sentence a turn ends with when no usable answer ever arrived.
///
/// A provider that stops at the request's output budget answers with content,
/// not with a transport failure, so the only trace of the cause is the
/// truncation itself. The bound that cut the answer is the operator's next
/// action, so the refusal names it.
fn answer_refusal(refusal: &str, cut_off: bool, max_tokens: Option<u32>) -> String {
    match max_tokens.filter(|_| cut_off) {
        Some(tokens) => {
            format!("{refusal}; the answer was cut off by the output budget of {tokens} tokens")
        }
        None => refusal.to_string(),
    }
}

impl Conversation {
    /// One bounded correction for an answer that never arrived usable.
    ///
    /// The model receives the exact refusal once inside the same turn.
    /// Exhausting the correction budget ends the turn with the cause while
    /// leaving the retained assignment open.
    ///
    /// `Ok(None)` means a correction was asked for and the turn continues;
    /// `Ok(Some(refusal))` is the sentence the turn must end with.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn repair_unusable_answer(
        &mut self,
        args: &Args,
        step: u32,
        refusal: &str,
        cut_off: bool,
        answer_bytes: Option<usize>,
        repairs: &mut u32,
        hooks: &RunHooks<'_>,
    ) -> Result<Option<String>, String> {
        let repairable = *repairs < ANSWER_REPAIRS && args.max_steps.is_none_or(|max| step < max);
        // Only the provider knows whether it stopped for length
        // (`StreamErrorClass::Incomplete`); an answer it finished normally
        // with brackets open is malformed, not cut.
        let instruction = repair_instruction(refusal, cut_off, args.max_tokens);
        self.recorder.record(
            task_contract::VIOLATION_EVENT,
            json!({
                "step": step,
                "rule": ANSWER_RULE,
                "outcome": if repairable { "requested" } else { "rejected" },
                "message": refusal,
                "answerBytes": answer_bytes,
                "cutOff": cut_off,
                "prompt": repairable.then(|| instruction.clone()),
            }),
        )?;
        if !repairable {
            let refusal = answer_refusal(refusal, cut_off, args.max_tokens);
            self.recorder
                .record("run_error", json!({ "message": &refusal }))?;
            return Ok(Some(refusal));
        }
        *repairs += u32::from(true);
        hooks.note("model answer was not usable; asking for the complete answer");
        self.messages
            .push(json!({ "role": "user", "content": instruction }));
        Ok(None)
    }

    /// Everything a proposed final answer passes before the turn ends.
    /// `Ok(None)` means the turn continues instead of answering.
    pub(super) fn finish_answer(
        &mut self,
        args: &Args,
        step: u32,
        text: String,
        report: Option<Value>,
        prepared: &mut Prepared,
        hooks: &mut RunHooks,
    ) -> Result<Option<String>, String> {
        let report = if prepared.report_required {
            match task_contract::DeliveryReport::parse(report) {
                Ok(report) => Some(report),
                Err(reason) => {
                    let can_repair = args.max_steps.is_none_or(|max| step < max);
                    let instruction = format!("{reason}\n\n{}", task_contract::REPAIR_INSTRUCTION);
                    self.recorder.record(
                        task_contract::VIOLATION_EVENT,
                        json!({
                            "step": step,
                            "rule": "delivery-report",
                            "outcome": if can_repair { "requested" } else { "rejected" },
                            "message": reason,
                            "prompt": if can_repair { Some(&instruction) } else { None },
                        }),
                    )?;
                    if can_repair {
                        hooks.note("delivery report incomplete; requesting correction");
                        self.messages.push(json!({
                            "role": "user",
                            "content": instruction,
                        }));
                        return Ok(None);
                    }
                    let error = format!("Task contract not satisfied: {reason}");
                    self.recorder
                        .record("run_error", json!({ "message": error }))?;
                    return Err(if prepared.tracks_completion {
                        self.completion_failure("delivery_report", &error, hooks)
                    } else {
                        error
                    });
                }
            }
        } else {
            None
        };
        if prepared.tracks_completion
            && !self.verify_completion(
                args,
                &text,
                serde_json::to_value(&report).map_err(|error| error.to_string())?,
                hooks,
            )?
        {
            return Ok(None);
        }
        let blocked = report.as_ref().is_some_and(|report| report.is_blocked());
        let rendered = report
            .as_ref()
            .map(|report| report.render(&prepared.language));
        let text = match &rendered {
            Some(rendered) => format!("{}\n\n{}", text.trim_end(), rendered),
            None => text,
        };
        // Jeden measures the time to completion itself; the model states
        // neither figure in its report, so the answer cannot flatter it.
        let text = if prepared.tracks_completion {
            let state = crate::completion::read_state(&self.recorder.path())?;
            match crate::completion::timing::report(
                &state,
                prepared.started_at,
                prepared.language.code() == "pl",
            ) {
                Some(timing) => format!("{}\n\n{timing}", text.trim_end()),
                None => text,
            }
        } else {
            text
        };
        if self.answer_refused(args, step, &text, prepared, hooks)? {
            return Ok(None);
        }
        if let (Some(report), Some(rendered)) = (&report, &rendered) {
            self.recorder.record(
                "task_report",
                json!({
                    "version": task_contract::VERSION,
                    "status": if blocked { "blocked" } else { "complete" },
                    "report": report,
                    "text": rendered,
                }),
            )?;
        }
        if prepared.tracks_completion {
            crate::goal_lifecycle::finish_verified_goal(
                &args.cwd,
                &self.recorder.path(),
                hooks.goal_event.as_ref(),
                prepared.classification.take(),
            )?;
        }
        self.recorder.record(
            "final",
            json!({ "step": step, "text": text, "taskStatus": if blocked { "blocked" } else { "complete" } }),
        )?;
        // Persist the user-visible answer (not the raw JSON action blob) so the
        // next turn's context is clean.
        if let Some(last) = self.messages.last_mut() {
            last["content"] = json!(text);
        }
        // When plan mode is on, the final answer IS the plan; persist it so
        // `/plan-review` can surface it.
        capture_plan_if_enabled(&args.cwd, &text);
        let answer = self.maybe_advisor_review(args, text, hooks)?;
        let _ = self.maybe_auto_compact(args, hooks, "threshold", false)?;
        Ok(Some(answer))
    }

    /// The last gate before an answer is shown: the `Stop` hooks read the exact
    /// text the operator would receive. A refusal is never shown as the answer;
    /// it goes back to the model as the next instruction and the turn goes on,
    /// so an answer that a hook rejects (open work, a missing blocker id) is
    /// replaced by more work instead of reaching the operator first and being
    /// refused after. `max_steps`, when the operator set one, still bounds it.
    fn answer_refused(
        &mut self,
        args: &Args,
        step: u32,
        text: &str,
        prepared: &mut Prepared,
        hooks: &RunHooks,
    ) -> Result<bool, String> {
        let session = self.recorder.path();
        let session_id = session
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(reason) = crate::hooks::answer_stop_block(
            &args.cwd,
            &session_id,
            &session.join("transcript.jsonl"),
            text,
            prepared.stop_refused,
            args.allow_command,
        ) else {
            return Ok(false);
        };
        prepared.stop_refused = true;
        let completion_state = if prepared.tracks_completion {
            Some(crate::completion::read_state(&session)?)
        } else {
            None
        };
        self.recorder.record(
            task_contract::VIOLATION_EVENT,
            json!({
                "step": step,
                "rule": STOP_RULE,
                "outcome": "requested",
                "message": &reason,
                "prompt": &reason,
                "completion": completion_state.as_ref().map(crate::completion::snapshot_value),
            }),
        )?;
        if let Some(state) = completion_state.as_ref() {
            self.publish_completion(state, hooks)?;
            self.recorder.record(
                "completion_stop_refusal",
                json!({
                    "step": step,
                    "reason": &reason,
                    "status": state.status(),
                    "openTasks": state.tasks.iter().filter(|task| !task.status.terminal()).collect::<Vec<_>>(),
                    "openRequests": crate::completion::open_asks(state, &session)?,
                }),
            )?;
        }
        hooks.note("answer refused by a Stop hook; continuing the work");
        self.messages
            .push(json!({ "role": "user", "content": reason }));
        Ok(true)
    }
}
