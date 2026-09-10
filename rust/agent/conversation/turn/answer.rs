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

fn repair_instruction(refusal: &str, cut_off: bool, max_tokens: Option<u32>) -> String {
    let budget = match max_tokens {
        Some(tokens) => format!(" of {tokens} tokens"),
        None => String::new(),
    };
    let opening = if cut_off {
        "Your previous answer stopped before it was complete"
    } else {
        "Your previous answer could not be read as an action"
    };
    format!(
        "{opening}: {refusal}\n\nSend the whole answer again as exactly one complete JSON object of the action protocol, with nothing before or after it, and keep it short enough to finish inside the output budget{budget}."
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
    /// A cut-off or unreadable answer used to end the turn where it was read.
    /// In a completion-managed conversation that ended the whole assignment:
    /// on 2026-09-10 a provider truncated one intake answer mid-string and the
    /// user's retained request stopped at `Work remains open (task_intake):
    /// EOF while parsing a string at line 1 column 1440`. The model now gets
    /// the exact refusal back once, inside the same turn, and only an
    /// exhausted correction budget ends it - with a sentence that says what
    /// happened to the answer.
    ///
    /// `Ok(None)` means a correction was asked for and the turn continues;
    /// `Ok(Some(refusal))` is the sentence the turn must end with.
    pub(super) fn repair_unusable_answer(
        &mut self,
        args: &Args,
        step: u32,
        refusal: &str,
        answer_bytes: Option<usize>,
        repairs: &mut u32,
        hooks: &RunHooks<'_>,
    ) -> Result<Option<String>, String> {
        let repairable = *repairs < ANSWER_REPAIRS && args.max_steps.is_none_or(|max| step < max);
        let cut_off =
            crate::protocol::is_incomplete_answer(refusal) || is_incomplete_output_error(refusal);
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
        let text = if let Some(report) = &report {
            let rendered = report.render(&prepared.language);
            self.recorder.record(
                "task_report",
                json!({
                    "version": task_contract::VERSION,
                    "status": if blocked { "blocked" } else { "complete" },
                    "report": report,
                    "text": rendered,
                }),
            )?;
            format!("{}\n\n{}", text.trim_end(), rendered)
        } else {
            text
        };
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
}
