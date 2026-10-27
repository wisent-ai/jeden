//! What a turn does when the model call itself fails: promote the route out of
//! a context overflow, recover a cut-off answer from a compacted history, or
//! ask for an answer that fits. `Ok(())` means the turn continues.

use super::super::*;

/// Whether the conversation holds more than its one prompt: history a
/// compaction can cut.
fn has_history(messages: &[Value]) -> bool {
    let mut spoken = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) != Some("system"));
    spoken.next().is_some() && spoken.next().is_some()
}

impl Conversation {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn recover_stream_failure(
        &mut self,
        args: &Args,
        step: u32,
        failure: crate::model_router::StreamFailure,
        prepared: &mut super::prompt::Prepared,
        repairs: &mut super::answer::AnswerRepairs,
        hooks: &mut RunHooks,
    ) -> Result<(), String> {
        let router = &mut prepared.router;
        let error = failure.message;
        for result in &failure.route_results {
            self.recorder
                .record(
                    "model_route_result",
                    json!({ "step": step, "result": result }),
                )
                .map_err(|record_error| {
                    format!("{error}; recording model_route_result failed: {record_error}")
                })?;
        }
        let overflow = failure.class == crate::model_router::StreamErrorClass::ContextOverflow;
        if overflow && !failure.visible_output {
            // The promotions are tried in order; those not tried stay queued.
            let mut promotions = std::mem::take(&mut router.context_promotions).into_iter();
            while let Some(next) = promotions.next() {
                let current = crate::model_router::RouteDescriptor {
                    model: router.model.clone(),
                    service_tier: Some(router.service_tier.clone())
                        .filter(|tier| !tier.trim().is_empty()),
                };
                if next == current {
                    continue;
                }
                router.context_promotions = promotions.collect();
                router.model = next.model.clone();
                // The router states no tier as an empty one.
                router.service_tier = match next.service_tier.clone() {
                    Some(tier) => tier,
                    None => String::new(),
                };
                let result = crate::model_router::RouteResult::RouteChanged {
                    from: current,
                    to: next,
                    reason: "context promotion".into(),
                };
                self.recorder.record(
                    "model_route_result",
                    json!({ "step": step, "result": result }),
                ).map_err(|record_error| {
                    format!("{error}; recording context promotion model_route_result failed: {record_error}")
                })?;
                hooks.note("context overflow: promoted model route");
                return Ok(());
            }
        }
        let cut_off = failure.class == crate::model_router::StreamErrorClass::Incomplete;
        let recovery_reason = if overflow {
            Some("overflow")
        } else if cut_off {
            Some("incomplete")
        } else {
            None
        };
        if let Some(reason) =
            recovery_reason.filter(|_| !failure.visible_output && has_history(&self.messages))
        {
            self.recorder.record(
                "run_error",
                json!({ "message": error, "recovering": true, "reason": reason }),
            ).map_err(|record_error| {
                format!("{error}; recording run_error before context {reason} recovery failed: {record_error}")
            })?;
            let instructions = format!("Automatic {reason} recovery. Preserve the active user request, decisions, files, tool results, and next action.");
            match self.compact(args, &instructions, hooks) {
                Ok(_) => {
                    let prompt = "Continue the interrupted turn from the compacted summary. Do not repeat completed work; take the next required action.";
                    self.recorder.record(
                        "auto_continue",
                        json!({ "reason": reason, "prompt": prompt }),
                    ).map_err(|record_error| {
                        format!("{error}; recording auto_continue after context {reason} recovery failed: {record_error}")
                    })?;
                    self.messages
                        .push(json!({ "role": "user", "content": prompt }));
                    return Ok(());
                }
                Err(recovery_error) => {
                    let refusal =
                        format!("{error}; context {reason} recovery failed: {recovery_error}");
                    self.recorder
                        .record(
                            "auto_compaction_error",
                            json!({ "reason": reason, "error": recovery_error }),
                        )
                        .map_err(|record_error| {
                            format!(
                                "{refusal}; recording auto_compaction_error failed: {record_error}"
                            )
                        })?;
                    return Err(refusal);
                }
            }
        }
        // A cut-off first response has no history to compact. Its observed
        // provider failure participates in repeated-failure detection.
        if cut_off && !failure.visible_output {
            let refusal = format!("model answer was cut off before it was complete ({error})");
            let Some(refusal) = self
                .repair_unusable_answer(
                    args,
                    step,
                    &refusal,
                    super::answer::AnswerFailure::CutOff(&error),
                    repairs,
                    hooks,
                )
                .map_err(|recovery_error| {
                    format!("{refusal}; answer recovery failed: {recovery_error}")
                })?
            else {
                return Ok(());
            };
            return Err(if prepared.tracks_completion {
                self.completion_failure("model_answer", &refusal, hooks)
            } else {
                refusal
            });
        }
        self.recorder
            .record("run_error", json!({ "message": error }))
            .map_err(|record_error| {
                format!("{error}; recording run_error failed: {record_error}")
            })?;
        Err(if prepared.tracks_completion {
            self.completion_failure("model_request", &error, hooks)
        } else {
            error
        })
    }
}
