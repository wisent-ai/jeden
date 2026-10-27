//! One turn of a conversation: prepare the request, then take one model action
//! per step until an answer passes its contract and its acceptance check.
//!
//! An answer that cannot be read is corrected once here instead of ending the
//! turn, because in a completion-managed conversation the end of a turn is the
//! end of the user's retained assignment.

use super::*;

mod answer;
mod failure;
mod prompt;
mod step;
mod tools;

/// The steps a turn has begun, and when each began. A step's number is how many
/// have begun, so the first step is the first without a counter to seed, and the
/// step budget is the most that may begin.
struct Steps {
    began: Vec<std::time::Instant>,
    limit: Option<u32>,
}

impl Steps {
    /// Begin the next step: its number, or `None` when the budget is spent.
    fn begin(&mut self) -> Option<u32> {
        self.began.push(std::time::Instant::now());
        let number = match u32::try_from(self.began.len()) {
            Ok(number) => number,
            Err(_past_any_budget) => return None,
        };
        if self.limit.is_some_and(|limit| number > limit) {
            self.began.pop();
            return None;
        }
        Some(number)
    }

    /// Whether the step under way is the turn's first.
    fn first(&self) -> bool {
        matches!(self.began.as_slice(), [_])
    }
}

impl Conversation {
    pub(crate) fn run_turn(
        &mut self,
        args: &Args,
        task: &str,
        attachments: &[crate::model_router::ModelAttachment],
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        let tracks_completion = self.tracks_completion(args);
        let continuing = std::mem::take(&mut self.continuation);
        self.recorder.ensure()?;
        let completion_request = if tracks_completion && !continuing {
            Some(self.capture_completion(args, task, hooks)?)
        } else {
            None
        };
        let mut prepared = self
            .prepare_turn(
                args,
                task,
                continuing,
                tracks_completion,
                completion_request,
                hooks,
            )
            .map_err(|error| {
                match self.recorder.record(
                    "run_error",
                    json!({ "operation": "prepare_turn", "message": error }),
                ) {
                    Ok(()) => error,
                    Err(record_error) => {
                        format!("{error}; recording startup failure failed: {record_error}")
                    }
                }
            })?;
        let mut tool_specs = if args.model_only {
            Vec::new()
        } else {
            rust_tool_specs(&args.cwd)
        };
        if self.inspection {
            tool_specs.retain(|spec| {
                spec.pointer("/function/name")
                    .and_then(Value::as_str)
                    .is_some_and(crate::agent::is_verification_read_tool)
            });
        }
        let step_max_label = match args.max_steps {
            Some(max) => max.to_string(),
            None => "unbounded".to_string(),
        };
        let mut steps = Steps {
            began: Vec::new(),
            limit: args.max_steps,
        };
        let mut repairs = answer::AnswerRepairs::new();
        // A `Stop` hook reads the finished answer and may refuse it. Streaming
        // the text first showed the operator an answer the hook then refused,
        // so the turn looked ended while the work went on; with a `Stop` hook
        // configured the text waits for `answer_refused` like a report does.
        let stop_gated =
            crate::hooks::has_event_hooks(&args.cwd, crate::hooks::event::STOP, args.allow_command);
        // A model call that never produced an answer is not a step of this
        // turn's work: it is the transport failing. Advancing the step counter
        // on it spent an operator's whole budget on a flapping gateway and
        // ended the assignment with `max steps exceeded`, a sentence about the
        // budget rather than about the connection that kept dropping. The step
        // advances only once an answer has been read. Recovery proceeds only
        // after changing the route, compacting history or repairing an answer;
        // an unrecoverable failure ends the turn with its original cause.
        let mut current = steps.begin();
        'steps: while let Some(step) = current {
            if tracks_completion {
                self.refresh_completion_context(hooks)?;
            }
            if hooks.cancelled() {
                return Err(self.cancelled_turn(tracks_completion, hooks)?);
            }
            hooks.note(&format!("thinking (step {}/{})", step, step_max_label));
            // The chat memory replaces compaction: with it on, history is
            // never cut, and the view a step sends keeps its size by itself.
            if !steps.first() && self.memory.is_none() {
                self.maybe_auto_compact(args, hooks, "threshold", true)?;
            }
            if !args.model_only {
                self.restore_session_context(args)?;
            }
            let step_messages = self.step_messages(&prepared.config, args)?;
            let outbound_messages =
                prepare_outbound_messages(&args.cwd, &step_messages, attachments)?;
            let streaming = match step::call(
                &prepared.router,
                outbound_messages,
                args.max_tokens.map(|tokens| tokens as usize),
                &tool_specs,
                prepared.report_required || tracks_completion || stop_gated,
                hooks,
            ) {
                Ok(streaming) => streaming,
                Err(failure) => {
                    self.recover_stream_failure(
                        args,
                        step,
                        failure,
                        &mut prepared,
                        &mut repairs,
                        hooks,
                    )?;
                    continue 'steps;
                }
            };
            current = steps.begin();
            let content = self.record_step(args, step, streaming, &mut prepared)?;
            self.build_memory_ahead()?;
            if args.model_only {
                self.messages
                    .push(json!({ "role": "assistant", "content": content.clone() }));
                self.recorder
                    .record("final", json!({ "step": step, "text": content.clone() }))?;
                return Ok(content);
            }
            let action = match action_or_text(&content, self.inspection) {
                Ok(action) => action,
                Err(refusal) => {
                    // Retain the exact response for both the correction prompt
                    // and repeated-failure detection.
                    self.messages
                        .push(json!({ "role": "assistant", "content": content.clone() }));
                    let Some(refusal) = self
                        .repair_unusable_answer(
                            args,
                            step,
                            &refusal,
                            answer::AnswerFailure::Malformed(&content),
                            &mut repairs,
                            hooks,
                        )
                        .map_err(|recovery_error| {
                            format!("{refusal}; answer recovery failed: {recovery_error}")
                        })?
                    else {
                        continue 'steps;
                    };
                    return Err(if tracks_completion {
                        self.completion_failure("model_answer", &refusal, hooks)
                    } else {
                        refusal
                    });
                }
            };
            self.recorder.record(
                "action",
                json!({ "step": step, "action": action_to_value(&action) }),
            )?;
            self.messages
                .push(json!({ "role": "assistant", "content": content }));

            match action {
                Action::Final { text, report } => {
                    if let Some(answer) =
                        self.finish_answer(args, step, text, report, &mut prepared, hooks)?
                    {
                        return Ok(answer);
                    }
                }
                Action::Message { text } => {
                    if self.answer_refused(args, step, &text, &mut prepared, hooks)? {
                        continue 'steps;
                    }
                    self.recorder
                        .record("assistant_message", json!({ "step": step, "text": text }))?;
                    hooks.trace(&TraceEvent::Message { text: &text });
                    if let Some(last) = self.messages.last_mut() {
                        last["content"] = json!(text);
                    }
                }
                Action::Tool { tool, input } => {
                    let result = self.handle_tool_action(
                        args,
                        step,
                        &ToolAction { tool, input },
                        tracks_completion,
                        hooks,
                    )?;
                    self.messages.push(json!({
                        "role": "user",
                        "content": crate::tool_runtime::format_tool_result(&result),
                    }));
                }
                Action::Tools { tools } => {
                    let mut results = Vec::with_capacity(tools.len());
                    for tool in &tools {
                        results.push(self.handle_tool_action(
                            args,
                            step,
                            tool,
                            tracks_completion,
                            hooks,
                        )?);
                    }
                    self.messages.push(json!({
                        "role": "user",
                        "content": crate::tool_runtime::format_tool_result(&json!(results)),
                    }));
                }
            }
        }

        let err = "max steps exceeded".to_string();
        self.recorder
            .record("run_error", json!({ "message": err }))?;
        Err(if tracks_completion {
            self.completion_failure("execution_limit", &err, hooks)
        } else {
            err
        })
    }

    /// The recorded refusal a cancelled turn answers with, in the one shape
    /// every cancellation point shares.
    fn cancelled_turn(
        &mut self,
        tracks_completion: bool,
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        let err = "Turn cancelled.".to_string();
        self.recorder
            .record("run_error", json!({ "message": err }))?;
        Ok(if tracks_completion {
            self.completion_failure("turn_cancelled", &err, hooks)
        } else {
            err
        })
    }
}
