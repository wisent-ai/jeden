use super::*;

mod advisor;

impl Conversation {
    pub(in crate::agent) fn maybe_auto_compact(
        &mut self,
        args: &Args,
        hooks: &mut RunHooks,
        reason: &str,
        add_continue_prompt: bool,
    ) -> Result<bool, String> {
        let Some(threshold) = Self::auto_compaction_threshold()? else {
            return Ok(false);
        };
        let mut tokens = self.approx_tokens();
        if tokens < threshold || self.turn_len() == 0 {
            return Ok(false);
        }
        self.prune_tool_results_if_needed(threshold)?;
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
            ).map_err(|record_error| {
                format!("auto-compaction ({reason}) failed: {error}; recording the failure also failed: {record_error}")
            })?;
            return Err(format!("auto-compaction ({reason}) failed: {error}"));
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
        self.restore_session_context(args)?;
        hooks.note("compacting conversation");
        let config = load_config(&args.cwd);
        let router = self.session_router(&config, args)?;
        let transcript = transcript_text(&self.messages);
        let extra = if instructions.trim().is_empty() {
            String::new()
        } else {
            format!("\nFocus the summary on: {}", instructions.trim())
        };
        let ask = vec![
            json!({ "role": "system", "content": "You compress a coding-agent conversation into a durable brief. Reply with plain text only." }),
            json!({ "role": "user", "content": format!("Summarize the conversation below so work can continue with full context but far fewer tokens. Preserve decisions, file paths, open tasks, and constraints.{}\n\n---\n{}", extra, transcript) }),
        ];
        let ask = prepare_outbound_messages(&args.cwd, &ask, &[])?;
        let summary_completion =
            chat_completion(&router, ask, args.max_tokens.map(|t| t as usize), &[])?;
        if let Some(usage) = &summary_completion.usage {
            if let Err(error) = append_usage_event(
                &args.cwd,
                &router,
                usage,
                usage_cost(&args.cwd, &config, &router.model, usage),
                None,
                None,
            ) {
                hooks.note(&format!("compaction summary usage not recorded: {error}"));
            }
        }
        let summary = summary_completion.content;
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        let memory = crate::memory::MemoryStore::open()?;
        let scope = crate::memory::MemoryScope {
            kind: "repo".into(),
            id: args.cwd.display().to_string(),
        };
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
        self.session_context_pending = Some("compact");
        self.restore_session_context(args)?;
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
        self.restore_session_context(args)?;
        hooks.note("generating handoff");
        let config = load_config(&args.cwd);
        let router = self.session_router(&config, args)?;
        let transcript = transcript_text(&self.messages);
        let extra = if focus.trim().is_empty() {
            String::new()
        } else {
            format!("\nFocus the handoff on: {}", focus.trim())
        };
        let ask = vec![
            json!({ "role": "system", "content": "You write a handoff brief so a fresh agent session can continue this work with no prior context. Reply with a concise plain-text brief: goal, decisions, files touched, open tasks, next steps." }),
            json!({ "role": "user", "content": format!("Write the handoff brief for the conversation below.{}\n\n---\n{}", extra, transcript) }),
        ];
        let ask = prepare_outbound_messages(&args.cwd, &ask, &[])?;
        let brief_completion =
            chat_completion(&router, ask, args.max_tokens.map(|t| t as usize), &[])?;
        if let Some(usage) = &brief_completion.usage {
            if let Err(error) = append_usage_event(
                &args.cwd,
                &router,
                usage,
                usage_cost(&args.cwd, &config, &router.model, usage),
                None,
                None,
            ) {
                hooks.note(&format!("handoff brief usage not recorded: {error}"));
            }
        }
        let brief = brief_completion.content;
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        let artifact_dir = self.recorder.artifact_dir();
        fs::create_dir_all(&artifact_dir).map_err(|error| {
            format!(
                "handoff could not create artifact directory {}: {error}",
                artifact_dir.display()
            )
        })?;
        let file = artifact_dir.join("handoff.md");
        let doc = if focus.trim().is_empty() {
            brief.clone()
        } else {
            format!("Focus: {}\n\n{}", focus.trim(), brief)
        };
        fs::write(&file, &doc).map_err(|error| {
            format!("handoff could not write brief {}: {error}", file.display())
        })?;
        self.recorder.record(
            "handoff",
            json!({ "focus": focus, "brief": brief.clone(), "file": file }),
        )?;
        let parent_session = self.recorder.path();
        let parent_entry = self.recorder.active_leaf()?;
        let messages = vec![
            json!({ "role": "system", "content": system_prompt_checked(&args.cwd)? }),
            json!({ "role": "system", "content": format!("Handoff brief from the prior session:\n{}", brief) }),
        ];
        let recorder =
            super::history::prepare_child(&args.cwd, &parent_session, parent_entry, "handoff")?;
        self.recorder = recorder;
        self.reconcile_completion = true;
        self.messages = messages;
        self.session_context_pending = Some("handoff");
        self.restore_session_context(args)?;
        self.recorder
            .record_context("handoff_seed", &self.messages)?;
        Ok(format!(
            "Handoff brief written to {} and a fresh session was started seeded with it.\n\n{}",
            file.display(),
            brief
        ))
    }
}

/// The conversation after its system prompt, one line per message, as the
/// compaction and handoff prompts read it. A message whose content is not
/// plain text (tool calls, multi-part content) is given as its JSON rather
/// than dropped, and a message without a role or content says so; they were
/// rendered as "?" and an empty line, so a summary lost what they held.
fn transcript_text(messages: &[Value]) -> String {
    let Some((_, conversation)) = messages.split_first() else {
        return String::new();
    };
    conversation
        .iter()
        .map(|message| {
            let role = match message.get("role").and_then(Value::as_str) {
                Some(role) => role,
                None => "unattributed",
            };
            let content = match message.get("content") {
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None => "(no content)".to_string(),
            };
            format!("{role}: {content}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
