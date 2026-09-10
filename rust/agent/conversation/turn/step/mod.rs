//! One step of a turn: the model call, the live text sink a surface may see,
//! and the durable record of the route, the spend and the raw answer.

use super::super::super::*;

/// Ask the model for one action.
///
/// Deltas stream out only when this turn has no answer to withhold, and never
/// while the model is emitting a raw action object: the buffer holds output
/// until the first non-whitespace character decides prose or JSON, so protocol
/// syntax cannot leak into a surface.
pub(super) fn call(
    router: &crate::model_router::ChatConfig,
    messages: Vec<Value>,
    max_tokens: Option<usize>,
    tool_specs: &[Value],
    withhold_answer: bool,
    hooks: &RunHooks<'_>,
) -> Result<crate::model_router::StreamingCompletion, crate::model_router::StreamFailure> {
    let decided = std::cell::Cell::new(false);
    let suppress = std::cell::Cell::new(false);
    let pending = std::cell::RefCell::new(String::new());
    let mut on_delta = |piece: &str| -> bool {
        // Do not expose an answer before its delivery report is checked.
        if withhold_answer {
            return false;
        }
        if !decided.get() {
            pending.borrow_mut().push_str(piece);
            let mut buffered = pending.borrow_mut();
            let lead = buffered.trim_start();
            if lead.is_empty() {
                return false;
            }
            decided.set(true);
            suppress.set(lead.starts_with('{') || lead.starts_with('['));
            if !suppress.get() {
                hooks.push_delta(&buffered);
            }
            let visible = !suppress.get();
            buffered.clear();
            return visible;
        }
        if suppress.get() {
            false
        } else {
            hooks.push_delta(piece);
            true
        }
    };
    let mut on_reasoning = |piece: &str| hooks.trace(&TraceEvent::Reasoning { text: piece });
    chat_completion_streaming(
        router,
        messages,
        max_tokens,
        tool_specs,
        &mut on_delta,
        &mut on_reasoning,
        &|| hooks.cancelled(),
    )
}

impl Conversation {
    /// Record which route served one step, what it spent, and the answer it
    /// produced; the route the model actually landed on carries to the next
    /// step. Returns the raw answer content.
    pub(super) fn record_step(
        &mut self,
        args: &Args,
        step: u32,
        streaming: crate::model_router::StreamingCompletion,
        prepared: &mut super::prompt::Prepared,
    ) -> Result<String, String> {
        for result in &streaming.route_results {
            self.recorder.record(
                "model_route_result",
                json!({ "step": step, "result": result }),
            )?;
        }
        if let Some(target) = &streaming.subscription_target {
            self.recorder.record(
                "model_subscription_route",
                json!({
                    "step": step,
                    "decisionId": streaming.subscription_decision_id.as_deref(),
                    "providerId": target.provider_id.as_str(),
                    "accountId": target.account_id.as_str(),
                    "subscriptionId": target.subscription_id.as_str(),
                    "quotaBucket": target.quota_bucket.as_str(),
                }),
            )?;
        }
        prepared.router.model = streaming.route.model.clone();
        prepared.router.service_tier = streaming.route.service_tier.clone().unwrap_or_default();
        if let Some(usage) = &streaming.completion.usage {
            if let Err(error) = append_usage_event(
                &args.cwd,
                &prepared.router,
                usage,
                usage_cost(&args.cwd, &prepared.config, &prepared.router.model, usage),
                streaming.subscription_target.as_ref(),
                streaming.subscription_decision_id.as_deref(),
            ) {
                self.recorder
                    .record("usage_error", json!({ "message": error }))
                    .ok();
            }
        }
        let content = streaming.completion.content;
        self.recorder.record(
            "assistant_raw",
            json!({ "step": step, "content": content.clone() }),
        )?;
        Ok(content)
    }
}
