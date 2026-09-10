//! What a turn does when the model call itself fails: promote the route out of
//! a context overflow, recover a cut-off answer from a compacted history, or
//! ask once for an answer that fits. `Ok(())` means the turn continues.

use super::super::*;

impl Conversation {
    pub(super) fn recover_stream_failure(
        &mut self,
        args: &Args,
        step: u32,
        failure: crate::model_router::StreamFailure,
        prepared: &mut super::prompt::Prepared,
        repairs: &mut u32,
        hooks: &mut RunHooks,
    ) -> Result<(), String> {
        let router = &mut prepared.router;
        for result in &failure.route_results {
            self.recorder.record(
                "model_route_result",
                json!({ "step": step, "result": result }),
            )?;
        }
        let error = failure.message;
        let overflow = failure.class == crate::model_router::StreamErrorClass::ContextOverflow
            || is_context_overflow_error(&error);
        if overflow && !failure.visible_output {
            while !router.context_promotions.is_empty() {
                let next = router.context_promotions.remove(0);
                let current = crate::model_router::RouteDescriptor {
                    model: router.model.clone(),
                    service_tier: (!router.service_tier.trim().is_empty())
                        .then(|| router.service_tier.clone()),
                };
                if next == current {
                    continue;
                }
                router.model = next.model.clone();
                router.service_tier = next.service_tier.clone().unwrap_or_default();
                let result = crate::model_router::RouteResult::RouteChanged {
                    from: current,
                    to: next,
                    reason: "context promotion".into(),
                };
                self.recorder.record(
                    "model_route_result",
                    json!({ "step": step, "result": result }),
                )?;
                hooks.note("context overflow: promoted model route");
                return Ok(());
            }
        }
        let cut_off = is_incomplete_output_error(&error);
        let recovery_reason = if overflow {
            Some("overflow")
        } else if cut_off {
            Some("incomplete")
        } else {
            None
        };
        if let Some(reason) =
            recovery_reason.filter(|_| !failure.visible_output && self.turn_len() > 1)
        {
            self.recorder.record(
                "run_error",
                json!({ "message": error, "recovering": true, "reason": reason }),
            )?;
            let instructions = format!("Automatic {reason} recovery. Preserve the active user request, decisions, files, tool results, and next action.");
            match self.compact(args, &instructions, hooks) {
                Ok(_) => {
                    let prompt = "Continue the interrupted turn from the compacted summary. Do not repeat completed work; take the next required action.";
                    self.recorder.record(
                        "auto_continue",
                        json!({ "reason": reason, "prompt": prompt }),
                    )?;
                    self.messages
                        .push(json!({ "role": "user", "content": prompt }));
                    return Ok(());
                }
                Err(recovery_error) => {
                    self.recorder.record(
                        "auto_compaction_error",
                        json!({ "reason": reason, "error": recovery_error }),
                    )?;
                    return Err(format!(
                        "Context {} recovery failed: {}",
                        reason, recovery_error
                    ));
                }
            }
        }
        // A first answer cut off by the output budget has no history to
        // compact, and it used to end the turn - and with it a whole retained
        // assignment - on the gateway's own `model response incomplete:
        // length`. The budget is a bound on one answer, not on the work, so
        // the model is asked once for an answer that fits.
        if cut_off && !failure.visible_output {
            // The budget itself is named once, where every unusable answer is
            // refused, so a caller never reads it twice.
            let refusal = format!("model answer was cut off before it was complete ({error})");
            let Some(refusal) =
                self.repair_unusable_answer(args, step, &refusal, None, repairs, hooks)?
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
            .record("run_error", json!({ "message": error }))?;
        Err(if prepared.tracks_completion {
            self.completion_failure("model_request", &error, hooks)
        } else {
            error
        })
    }
}
