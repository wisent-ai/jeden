//! The /advisor second pass over a turn's answer.
use super::*;

impl Conversation {
    /// If /advisor is enabled, run a second reviewer pass over the answer and
    /// append its critique. Best-effort: a reviewer failure never fails the turn.
    pub(in crate::agent::conversation) fn maybe_advisor_review(
        &mut self,
        args: &Args,
        answer: String,
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        let mut state = read_mode_state(&args.cwd)?;
        if state.pointer("/advisor/enabled").and_then(Value::as_bool) != Some(true) {
            return Ok(answer);
        }
        if hooks.cancelled() {
            return Ok(answer);
        }
        hooks.note("advisor review");
        let config = load_config(&args.cwd);
        let mut router = self.session_router(&config, args)?;
        if let Some(model) = state
            .pointer("/advisor/model")
            .and_then(Value::as_str)
            .filter(|m| !m.trim().is_empty())
        {
            router.model = model.to_string();
        }
        let ask = vec![
            json!({ "role": "system", "content": "You are a second-pass reviewer. Critique the assistant's answer for correctness, gaps, and risks in 2-4 concise bullet points. If it is sound, say so briefly. Reply with plain text only." }),
            json!({ "role": "user", "content": format!("Assistant answer to review:\n\n{}", answer) }),
        ];
        let ask = prepare_outbound_messages(&args.cwd, &ask, &[])?;
        match chat_completion(&router, ask, args.max_tokens.map(|t| t as usize), &[]) {
            Ok(review_completion) => {
                if let Some(usage) = &review_completion.usage {
                    if let Err(error) = append_usage_event(
                        &args.cwd,
                        &router,
                        usage,
                        usage_cost(&args.cwd, &config, &router.model, usage),
                        None,
                    ) {
                        hooks.note(&format!("advisor review usage not recorded: {error}"));
                    }
                }
                let review = review_completion.content;
                if let Err(error) = self
                    .recorder
                    .record("advisor", json!({ "review": review.clone() }))
                {
                    hooks.note(&format!(
                        "advisor review not recorded in the session: {error}"
                    ));
                }
                // Persist the review so `/advisor dump` can surface it.
                if let Some(obj) = state.as_object_mut() {
                    let advisor = obj.entry("advisor").or_insert_with(|| json!({}));
                    if let Some(advisor_obj) = advisor.as_object_mut() {
                        advisor_obj.insert(
                            "lastReview".into(),
                            json!({ "text": review.clone(), "at": now_stamp() }),
                        );
                    }
                    if let Err(error) = write_mode_state(&args.cwd, &state) {
                        hooks.note(&format!(
                            "advisor review not kept for /advisor dump: {error}"
                        ));
                    }
                }
                Ok(format!("{}\n\n— Advisor review —\n{}", answer, review))
            }
            Err(error) => Ok(format!(
                "{}\n\n(advisor review unavailable: {})",
                answer, error
            )),
        }
    }
}
