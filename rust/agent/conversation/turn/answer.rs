//! Accept proposed final answers after delivery and acceptance checks; correct
//! unusable answers while they change, and retain the cause when they repeat.

use super::super::*;
use super::prompt::Prepared;
use sha2::{Digest, Sha256};

mod stop;

pub(super) type AnswerRepairs = std::collections::BTreeSet<sha2::digest::Output<Sha256>>;

pub(super) enum AnswerFailure<'a> {
    Malformed(&'a str),
    CutOff(&'a str),
}

/// The rule name every unusable-answer correction is recorded under, beside
/// the delivery-report rule of the same event.
const ANSWER_RULE: &str = "model-answer";

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
    /// Corrections continue for distinct observed failures, within the caller's
    /// step limit. A repeated failure ends the turn with its recorded cause.
    ///
    /// `Ok(None)` means a correction was asked for and the turn continues;
    /// `Ok(Some(refusal))` is the sentence the turn must end with.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn repair_unusable_answer(
        &mut self,
        args: &Args,
        step: u32,
        refusal: &str,
        failure: AnswerFailure<'_>,
        repairs: &mut AnswerRepairs,
        hooks: &RunHooks<'_>,
    ) -> Result<Option<String>, String> {
        let (cut_off, observed, answer_bytes) = match failure {
            AnswerFailure::Malformed(answer) => (false, answer, Some(answer.len())),
            AnswerFailure::CutOff(cause) => (true, cause, None),
        };
        let mut identity = Sha256::new();
        identity.update([u8::from(cut_off)]);
        identity.update(observed.as_bytes());
        let repeated = !repairs.insert(identity.finalize());
        let repairable = !repeated && args.max_steps.is_none_or(|max| step < max);
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
                "repeatedFailure": repeated,
                "prompt": repairable.then(|| instruction.clone()),
            }),
        )?;
        if !repairable {
            let mut refusal = answer_refusal(refusal, cut_off, args.max_tokens);
            if repeated {
                refusal.push_str("; the same failure recurred after correction");
            }
            self.recorder
                .record("run_error", json!({ "message": &refusal }))?;
            return Ok(Some(refusal));
        }
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
}
