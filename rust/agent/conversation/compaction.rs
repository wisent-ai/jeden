use super::*;

impl Conversation {
    pub(in crate::agent) fn maybe_auto_compact(
        &mut self,
        args: &Args,
        hooks: &mut RunHooks,
        reason: &str,
        add_continue_prompt: bool,
    ) -> Result<bool, String> {
        let Some(threshold) = Self::auto_compaction_threshold() else {
            return Ok(false);
        };
        let mut tokens = self.approx_tokens();
        if tokens < threshold || self.turn_len() == 0 {
            return Ok(false);
        }
        let _ = self.prune_tool_results_if_needed(threshold)?;
        tokens = self.approx_tokens();
        if tokens < threshold {
            return Ok(false);
        }
        self.recorder.record(
            "auto_compaction",
            json!({ "reason": reason, "tokens": tokens, "threshold": threshold }),
        )?;
        let instructions = format!("Automatic {reason} compaction at ~{tokens} tokens (threshold {threshold}). Preserve the active user request, decisions, files, tool results, and next action.");
        if let Err(error) = self.compact(args, &instructions, hooks) {
            self.recorder.record(
                "auto_compaction_error",
                json!({ "reason": reason, "error": error }),
            )?;
            hooks.note(&format!("auto-compaction failed: {}", reason));
            return Ok(false);
        }
        if add_continue_prompt {
            let prompt = "Continue the interrupted turn from the compacted summary. Do not repeat completed work; take the next required action.";
            self.recorder.record(
                "auto_continue",
                json!({ "reason": reason, "prompt": prompt }),
            )?;
            self.messages
                .push(json!({ "role": "user", "content": prompt }));
        }
        Ok(true)
    }

    /// Summarize the live history into a single compact system note and drop the
    /// detailed turns — backs a real /compact instead of a mode-state flag.
    pub(crate) fn compact(
        &mut self,
        args: &Args,
        instructions: &str,
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        if self.turn_len() == 0 {
            return Err("Nothing to compact yet; the conversation is empty.".into());
        }
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        hooks.note("compacting conversation");
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        let transcript = self
            .messages
            .iter()
            .skip(1)
            .map(|m| {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                format!("{}: {}", role, content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let extra = if instructions.trim().is_empty() {
            String::new()
        } else {
            format!("\nFocus the summary on: {}", instructions.trim())
        };
        let ask = vec![
            json!({ "role": "system", "content": "You compress a coding-agent conversation into a durable brief. Reply with plain text only." }),
            json!({ "role": "user", "content": format!("Summarize the conversation below so work can continue with full context but far fewer tokens. Preserve decisions, file paths, open tasks, and constraints.{}\n\n---\n{}", extra, transcript) }),
        ];
        let ask = prepare_outbound_messages(&args.cwd, &ask)?;
        let summary_completion =
            chat_completion(&router, ask, args.max_tokens.map(|t| t as usize), &[])?;
        if let Some(usage) = &summary_completion.usage {
            let _ = append_usage_event(
                &args.cwd,
                &router,
                usage,
                usage_cost(&args.cwd, &config, &router.model, usage),
            );
        }
        let summary = summary_completion.content;
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        let memory = crate::memory::MemoryStore::open(crate::memory::MemoryStore::default_path())?;
        let scope = crate::memory::MemoryScope { kind: "repo".into(), id: args.cwd.display().to_string() };
        memory.persist_model_consolidation(&scope, &summary)?;
        let before = self.turn_len();
        self.recorder.record(
            "compaction",
            json!({ "before": before, "summary": summary.clone() }),
        )?;
        self.messages = vec![
            json!({ "role": "system", "content": system_prompt_checked(&args.cwd)? }),
            json!({ "role": "system", "content": format!("Prior conversation summary (compacted from {} messages):\n{}", before, summary) }),
        ];
        self.recorder.record_context("compaction", &self.messages)?;
        Ok(format!(
            "Compacted {} messages into a summary.\n\n{}",
            before, summary
        ))
    }

    /// Generate an LLM handoff brief from the live history, write it to the
    /// session artifacts, then start a fresh native session seeded with the
    /// brief instead of dumping the raw transcript.
    pub(crate) fn handoff(
        &mut self,
        args: &Args,
        focus: &str,
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        if self.turn_len() == 0 {
            return Err("Nothing to hand off yet; the conversation is empty.".into());
        }
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        hooks.note("generating handoff");
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        let transcript = self
            .messages
            .iter()
            .skip(1)
            .map(|m| {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                format!("{}: {}", role, content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let extra = if focus.trim().is_empty() {
            String::new()
        } else {
            format!("\nFocus the handoff on: {}", focus.trim())
        };
        let ask = vec![
            json!({ "role": "system", "content": "You write a handoff brief so a fresh agent session can continue this work with no prior context. Reply with a concise plain-text brief: goal, decisions, files touched, open tasks, next steps." }),
            json!({ "role": "user", "content": format!("Write the handoff brief for the conversation below.{}\n\n---\n{}", extra, transcript) }),
        ];
        let ask = prepare_outbound_messages(&args.cwd, &ask)?;
        let brief_completion =
            chat_completion(&router, ask, args.max_tokens.map(|t| t as usize), &[])?;
        if let Some(usage) = &brief_completion.usage {
            let _ = append_usage_event(
                &args.cwd,
                &router,
                usage,
                usage_cost(&args.cwd, &config, &router.model, usage),
            );
        }
        let brief = brief_completion.content;
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        let artifact_dir = self.recorder.artifact_dir();
        fs::create_dir_all(&artifact_dir).map_err(|e| e.to_string())?;
        let file = artifact_dir.join("handoff.md");
        let doc = if focus.trim().is_empty() {
            brief.clone()
        } else {
            format!("Focus: {}\n\n{}", focus.trim(), brief)
        };
        fs::write(&file, &doc).map_err(|e| e.to_string())?;
        self.recorder.record(
            "handoff",
            json!({ "focus": focus, "brief": brief.clone(), "file": file }),
        )?;
        let parent_session = self.recorder.path();
        let parent_entry = self.recorder.active_leaf()?;
        self.recorder = SessionRecorder::child(&args.cwd, parent_session, parent_entry);
        self.recorder.ensure()?;
        self.messages = vec![
            json!({ "role": "system", "content": system_prompt_checked(&args.cwd)? }),
            json!({ "role": "system", "content": format!("Handoff brief from the prior session:\n{}", brief) }),
        ];
        self.recorder.record_context("handoff_seed", &self.messages)?;
        Ok(format!(
            "Handoff brief written to {} and a fresh session was started seeded with it.\n\n{}",
            file.display(),
            brief
        ))
    }

    /// If /advisor is enabled, run a second reviewer pass over the answer and
    /// append its critique. Best-effort: a reviewer failure never fails the turn.
    pub(in crate::agent::conversation) fn maybe_advisor_review(
        &mut self,
        args: &Args,
        answer: String,
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        let mut state = read_mode_state(&args.cwd);
        if !state
            .pointer("/advisor/enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(answer);
        }
        if hooks.cancelled() {
            return Ok(answer);
        }
        hooks.note("advisor review");
        let config = load_config(&args.cwd);
        let mut router = model_router_config(&config, args);
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
        let ask = prepare_outbound_messages(&args.cwd, &ask)?;
        match chat_completion(&router, ask, args.max_tokens.map(|t| t as usize), &[]) {
            Ok(review_completion) => {
                if let Some(usage) = &review_completion.usage {
                    let _ = append_usage_event(
                        &args.cwd,
                        &router,
                        usage,
                        usage_cost(&args.cwd, &config, &router.model, usage),
                    );
                }
                let review = review_completion.content;
                self.recorder
                    .record("advisor", json!({ "review": review.clone() }))
                    .ok();
                // Persist the review so `/advisor dump` can surface it.
                if let Some(obj) = state.as_object_mut() {
                    let advisor = obj.entry("advisor").or_insert_with(|| json!({}));
                    if let Some(advisor_obj) = advisor.as_object_mut() {
                        advisor_obj.insert(
                            "lastReview".into(),
                            json!({ "text": review.clone(), "at": now_stamp() }),
                        );
                    }
                    let _ = write_mode_state(&args.cwd, &state);
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
