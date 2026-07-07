use super::*;

impl Conversation {
    pub(crate) fn run_turn(&mut self, args: &Args, task: &str, hooks: &mut RunHooks) -> Result<String, String> {
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        let mut effective_task = apply_mode_instructions(&args.cwd, task)?;
        let hook_context = crate::hooks::user_prompt_submit(&args.cwd, task, args.allow_command);
        if !hook_context.trim().is_empty() {
            effective_task = format!("{}\n\n[Hook context]\n{}", effective_task, hook_context.trim());
        }
        self.recorder.record(
            "user",
            json!({
                "task": effective_task,
                "cwd": args.cwd,
                "allowWrite": args.allow_write,
                "allowCommand": args.allow_command,
                "maxSteps": args.max_steps,
                "maxTokens": args.max_tokens,
            }),
        )?;

        let tool_specs = rust_tool_specs(&args.cwd);
        self.messages.push(json!({ "role": "user", "content": effective_task }));

        let step_max_label = args.max_steps.map(|m| m.to_string()).unwrap_or_else(|| "unbounded".to_string());
        let step_iter: Box<dyn Iterator<Item = u32>> = match args.max_steps {
            Some(max) => Box::new(u32::from(true)..=max),
            None => Box::new(u32::from(true)..),
        };
        for step in step_iter {
            if hooks.cancelled() {
                let err = "Turn cancelled.".to_string();
                self.recorder.record("run_error", json!({ "message": err }))?;
                return Err(err);
            }
            hooks.note(&format!("thinking (step {}/{})", step, step_max_label));
            if step > u32::from(true) {
                let _ = self.maybe_auto_compact(args, hooks, "threshold", true)?;
            }
            // Stream deltas, but suppress anything that looks like a raw JSON
            // action/tool blob so its syntax never leaks to the UI. Buffer until
            // the first non-whitespace character decides plain-text vs JSON.
            let decided = std::cell::Cell::new(false);
            let suppress = std::cell::Cell::new(false);
            let pending = std::cell::RefCell::new(String::new());
            let mut on_delta = |piece: &str| {
                if !decided.get() {
                    pending.borrow_mut().push_str(piece);
                    let buf = pending.borrow().clone();
                    let lead = buf.trim_start();
                    if lead.is_empty() {
                        return;
                    }
                    decided.set(true);
                    suppress.set(lead.starts_with('{') || lead.starts_with('['));
                    if !suppress.get() {
                        hooks.push_delta(&buf);
                    }
                    pending.borrow_mut().clear();
                    return;
                }
                if !suppress.get() {
                    hooks.push_delta(piece);
                }
            };
            let call = chat_completion_streaming(&router, self.messages.clone(), args.max_tokens.map(|t| t as usize), &tool_specs, &mut on_delta);
            match call {
                Ok(completion) => {
                    if let Some(usage) = &completion.usage {
                        if let Err(error) = append_usage_event(&args.cwd, &router, usage, usage_cost(&config, &router.model, usage)) {
                            self.recorder.record("usage_error", json!({ "message": error })).ok();
                        }
                    }
                    let content = completion.content;
                    self.recorder.record("assistant_raw", json!({ "step": step, "content": content.clone() }))?;
                    let action = action_or_text(&content)?;
                    self.recorder.record("action", json!({ "step": step, "action": action_to_value(&action) }))?;
                    self.messages.push(json!({ "role": "assistant", "content": content }));

                    match action {
                        Action::Final { text } => {
                            self.recorder.record("final", json!({ "step": step, "text": text }))?;
                            // Persist the user-visible answer (not the raw JSON
                            // action blob) so the next turn's context is clean.
                            if let Some(last) = self.messages.last_mut() {
                                last["content"] = json!(text);
                            }
                            // When plan mode is on, the final answer IS the plan;
                            // persist it so `/plan-review` can surface it.
                            capture_plan_if_enabled(&args.cwd, &text);
                            let answer = self.maybe_advisor_review(args, text, hooks)?;
                            let _ = self.maybe_auto_compact(args, hooks, "threshold", false)?;
                            return Ok(answer);
                        }
                        Action::Tool { tool, input } => {
                            if hooks.cancelled() {
                                let err = "Turn cancelled.".to_string();
                                self.recorder.record("run_error", json!({ "message": err }))?;
                                return Err(err);
                            }
                            let result = if let Some(reason) = crate::hooks::pretool_block(&args.cwd, &tool, &input, args.allow_command) {
                                hooks.note(&format!("tool blocked by hook: {}", tool));
                                json!({ "ok": false, "error": format!("blocked by PreToolUse hook: {}", reason) })
                            } else {
                                match resolve_tool_approval(args, &tool, &input, hooks) {
                                    ToolDecision::Allow { allow_write: aw, allow_command: ac } => {
                                        hooks.note(&format!("tool: {}", tool));
                                        let r = run_tool_action(args, &mut self.recorder, step, &ToolAction { tool: tool.clone(), input: input.clone() }, hooks.interactive, aw, ac)?;
                                        crate::hooks::posttool(&args.cwd, &tool, &r, args.allow_command);
                                        r
                                    }
                                    ToolDecision::Deny(reason) => {
                                        hooks.note(&format!("tool denied: {}", tool));
                                        json!({ "ok": false, "error": reason })
                                    }
                                }
                            };
                            self.messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(&result) }));
                        }
                        Action::Tools { tools } => {
                            let mut results = Vec::new();
                            for tool in tools {
                                if hooks.cancelled() {
                                    let err = "Turn cancelled.".to_string();
                                    self.recorder.record("run_error", json!({ "message": err }))?;
                                    return Err(err);
                                }
                                if let Some(reason) = crate::hooks::pretool_block(&args.cwd, &tool.tool, &tool.input, args.allow_command) {
                                    hooks.note(&format!("tool blocked by hook: {}", tool.tool));
                                    results.push(json!({ "ok": false, "error": format!("blocked by PreToolUse hook: {}", reason) }));
                                    continue;
                                }
                                match resolve_tool_approval(args, &tool.tool, &tool.input, hooks) {
                                    ToolDecision::Allow { allow_write: aw, allow_command: ac } => {
                                        hooks.note(&format!("tool: {}", tool.tool));
                                        let r = run_tool_action(args, &mut self.recorder, step, &tool, hooks.interactive, aw, ac)?;
                                        crate::hooks::posttool(&args.cwd, &tool.tool, &r, args.allow_command);
                                        results.push(r);
                                    }
                                    ToolDecision::Deny(reason) => {
                                        hooks.note(&format!("tool denied: {}", tool.tool));
                                        results.push(json!({ "ok": false, "error": reason }));
                                    }
                                }
                            }
                            self.messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(&json!(results)) }));
                        }
                    }
                }
                Err(error) => {
                    let recovery_reason = if is_context_overflow_error(&error) {
                        Some("overflow")
                    } else if is_incomplete_output_error(&error) {
                        Some("incomplete")
                    } else {
                        None
                    };
                    if let Some(reason) = recovery_reason.filter(|_| self.turn_len() > 1) {
                        self.recorder.record("run_error", json!({ "message": error, "recovering": true, "reason": reason }))?;
                        let instructions = format!("Automatic {reason} recovery. Preserve the active user request, decisions, files, tool results, and next action.");
                        match self.compact(args, &instructions, hooks) {
                            Ok(_) => {
                                let prompt = "Continue the interrupted turn from the compacted summary. Do not repeat completed work; take the next required action.";
                                self.recorder.record("auto_continue", json!({ "reason": reason, "prompt": prompt }))?;
                                self.messages.push(json!({ "role": "user", "content": prompt }));
                                continue;
                            }
                            Err(recovery_error) => {
                                self.recorder.record("auto_compaction_error", json!({ "reason": reason, "error": recovery_error }))?;
                                return Err(format!("Context {} recovery failed: {}", reason, recovery_error));
                            }
                        }
                    }
                    self.recorder.record("run_error", json!({ "message": error }))?;
                    return Err(error);
                }
            }
        }

        let err = "max steps exceeded".to_string();
        self.recorder.record("run_error", json!({ "message": err }))?;
        Err(err)
    }
}
