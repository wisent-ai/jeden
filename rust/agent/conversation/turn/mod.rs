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
        let mut prepared = self.prepare_turn(
            args,
            task,
            continuing,
            tracks_completion,
            completion_request,
            hooks,
        )?;
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
        let step_max_label = args
            .max_steps
            .map(|m| m.to_string())
            .unwrap_or_else(|| "unbounded".to_string());
        let step_iter: Box<dyn Iterator<Item = u32>> = match args.max_steps {
            Some(max) => Box::new(u32::from(true)..=max),
            None => Box::new(u32::from(true)..),
        };
        let mut repairs = u32::default();
        'steps: for step in step_iter {
            if tracks_completion {
                self.refresh_completion_context(hooks)?;
            }
            if hooks.cancelled() {
                return Err(self.cancelled_turn(tracks_completion, hooks)?);
            }
            hooks.note(&format!("thinking (step {}/{})", step, step_max_label));
            if step > u32::from(true) {
                let _ = self.maybe_auto_compact(args, hooks, "threshold", true)?;
            }
            let outbound_messages =
                prepare_outbound_messages(&args.cwd, &self.messages, attachments)?;
            let streaming = match step::call(
                &prepared.router,
                outbound_messages,
                args.max_tokens.map(|tokens| tokens as usize),
                &tool_specs,
                prepared.report_required || tracks_completion,
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
            let content = self.record_step(args, step, streaming, &mut prepared)?;
            if args.model_only {
                self.messages
                    .push(json!({ "role": "assistant", "content": content.clone() }));
                self.recorder
                    .record("final", json!({ "step": step, "text": content.clone() }))?;
                return Ok(content);
            }
            let action = match action_or_text(&content) {
                Ok(action) => action,
                Err(_) if self.inspection && serde_json::from_str::<Value>(&content).is_ok() => {
                    Action::Final {
                        text: content.clone(),
                        report: None,
                    }
                }
                Err(refusal) => {
                    // The answer is not an action this run can execute. Keep it
                    // in the history so the model reads what it sent, ask once
                    // for a whole answer, and end the turn only after that.
                    self.messages
                        .push(json!({ "role": "assistant", "content": content.clone() }));
                    let Some(refusal) = self.repair_unusable_answer(
                        args,
                        step,
                        &refusal,
                        Some(content.len()),
                        &mut repairs,
                        hooks,
                    )?
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
