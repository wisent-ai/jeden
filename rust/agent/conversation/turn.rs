use super::*;

impl Conversation {
    pub(crate) fn run_turn(
        &mut self,
        args: &Args,
        task: &str,
        attachments: &[crate::model_router::ModelAttachment],
        hooks: &mut RunHooks,
    ) -> Result<String, String> {
        let config = load_config(&args.cwd);
        let mut router = model_router_config(&config, args);
        let mut effective_task = if args.model_only {
            task.to_string()
        } else {
            apply_mode_instructions(&args.cwd, task)?
        };
        if !args.model_only {
            let hook_context =
                crate::hooks::user_prompt_submit(&args.cwd, task, args.allow_command);
            if !hook_context.trim().is_empty() {
                effective_task = format!(
                    "{}\n\n[Hook context]\n{}",
                    effective_task,
                    hook_context.trim()
                );
            }
            let extension_context = crate::hooks::extension_prompt_context(&args.cwd, task)?;
            if !extension_context.is_empty() {
                let mut sections = Vec::with_capacity(extension_context.len());
                for contribution in extension_context {
                    let mut section = format!(
                        "[{}:{}; precedence={}; source={}]\n{}",
                        contribution.kind,
                        contribution.id,
                        contribution.precedence,
                        contribution.source.display(),
                        contribution.content,
                    );
                    if !contribution.assets.is_empty() {
                        section.push_str("\nValidated assets:\n");
                        for asset in contribution.assets {
                            section.push_str("- ");
                            section.push_str(&asset.display().to_string());
                            section.push('\n');
                        }
                    }
                    sections.push(section);
                }
                effective_task.push_str("\n\n[Active extension rules and skills]\n");
                effective_task.push_str(&sections.join("\n\n"));
            }
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
                "modelOnly": args.model_only,
            }),
        )?;

        let tool_specs = if args.model_only {
            Vec::new()
        } else {
            rust_tool_specs(&args.cwd)
        };
        self.messages
            .push(json!({ "role": "user", "content": effective_task }));

        let step_max_label = args
            .max_steps
            .map(|m| m.to_string())
            .unwrap_or_else(|| "unbounded".to_string());
        let step_iter: Box<dyn Iterator<Item = u32>> = match args.max_steps {
            Some(max) => Box::new(u32::from(true)..=max),
            None => Box::new(u32::from(true)..),
        };
        'steps: for step in step_iter {
            if hooks.cancelled() {
                let err = "Turn cancelled.".to_string();
                self.recorder
                    .record("run_error", json!({ "message": err }))?;
                return Err(err);
            }
            hooks.note(&format!("thinking (step {}/{})", step, step_max_label));
            if step > u32::from(true) {
                let _ = self.maybe_auto_compact(args, hooks, "threshold", true)?;
            }
            let outbound_messages =
                prepare_outbound_messages(&args.cwd, &self.messages, attachments)?;
            // Stream deltas, but suppress anything that looks like a raw JSON
            // action/tool blob so its syntax never leaks to the UI. Buffer until
            // the first non-whitespace character decides plain-text vs JSON.
            let decided = std::cell::Cell::new(false);
            let suppress = std::cell::Cell::new(false);
            let pending = std::cell::RefCell::new(String::new());
            let mut on_delta = |piece: &str| -> bool {
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
            let call = chat_completion_streaming(
                &router,
                outbound_messages,
                args.max_tokens.map(|t| t as usize),
                &tool_specs,
                &mut on_delta,
                &|| hooks.cancelled(),
            );
            match call {
                Ok(streaming) => {
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
                    router.model = streaming.route.model.clone();
                    router.service_tier = streaming.route.service_tier.clone().unwrap_or_default();
                    let completion = streaming.completion;
                    if let Some(usage) = &completion.usage {
                        if let Err(error) = append_usage_event(
                            &args.cwd,
                            &router,
                            usage,
                            usage_cost(&args.cwd, &config, &router.model, usage),
                            streaming.subscription_target.as_ref(),
                            streaming.subscription_decision_id.as_deref(),
                        ) {
                            self.recorder
                                .record("usage_error", json!({ "message": error }))
                                .ok();
                        }
                    }
                    let content = completion.content;
                    self.recorder.record(
                        "assistant_raw",
                        json!({ "step": step, "content": content.clone() }),
                    )?;
                    if args.model_only {
                        self.messages
                            .push(json!({ "role": "assistant", "content": content.clone() }));
                        self.recorder
                            .record("final", json!({ "step": step, "text": content.clone() }))?;
                        return Ok(content);
                    }
                    let action = action_or_text(&content)?;
                    self.recorder.record(
                        "action",
                        json!({ "step": step, "action": action_to_value(&action) }),
                    )?;
                    self.messages
                        .push(json!({ "role": "assistant", "content": content }));

                    match action {
                        Action::Final { text } => {
                            self.recorder
                                .record("final", json!({ "step": step, "text": text }))?;
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
                                self.recorder
                                    .record("run_error", json!({ "message": err }))?;
                                return Err(err);
                            }
                            let result = if let Some(reason) = crate::hooks::pretool_block(
                                &args.cwd,
                                &tool,
                                &input,
                                args.allow_command,
                            ) {
                                hooks.note(&format!("tool blocked by hook: {}", tool));
                                json!({ "ok": false, "error": format!("blocked by PreToolUse hook: {}", reason) })
                            } else {
                                match resolve_tool_approval(args, &tool, &input, hooks) {
                                    ToolDecision::Allow {
                                        allow_write: aw,
                                        allow_command: ac,
                                    } => {
                                        hooks.note(&format!("tool: {}", tool));
                                        let r = run_tool_action(
                                            args,
                                            &mut self.recorder,
                                            step,
                                            &ToolAction {
                                                tool: tool.clone(),
                                                input: input.clone(),
                                            },
                                            hooks,
                                            aw,
                                            ac,
                                        )?;
                                        crate::hooks::posttool(
                                            &args.cwd,
                                            &tool,
                                            &r,
                                            args.allow_command,
                                        );
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
                                    self.recorder
                                        .record("run_error", json!({ "message": err }))?;
                                    return Err(err);
                                }
                                if let Some(reason) = crate::hooks::pretool_block(
                                    &args.cwd,
                                    &tool.tool,
                                    &tool.input,
                                    args.allow_command,
                                ) {
                                    hooks.note(&format!("tool blocked by hook: {}", tool.tool));
                                    results.push(json!({ "ok": false, "error": format!("blocked by PreToolUse hook: {}", reason) }));
                                    continue;
                                }
                                match resolve_tool_approval(args, &tool.tool, &tool.input, hooks) {
                                    ToolDecision::Allow {
                                        allow_write: aw,
                                        allow_command: ac,
                                    } => {
                                        hooks.note(&format!("tool: {}", tool.tool));
                                        let r = run_tool_action(
                                            args,
                                            &mut self.recorder,
                                            step,
                                            &tool,
                                            hooks,
                                            aw,
                                            ac,
                                        )?;
                                        crate::hooks::posttool(
                                            &args.cwd,
                                            &tool.tool,
                                            &r,
                                            args.allow_command,
                                        );
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
                Err(failure) => {
                    for result in &failure.route_results {
                        self.recorder.record(
                            "model_route_result",
                            json!({ "step": step, "result": result }),
                        )?;
                    }
                    let error = failure.message;
                    let overflow = failure.class
                        == crate::model_router::StreamErrorClass::ContextOverflow
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
                            continue 'steps;
                        }
                    }
                    let recovery_reason = if overflow {
                        Some("overflow")
                    } else if is_incomplete_output_error(&error) {
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
                                continue;
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
                    self.recorder
                        .record("run_error", json!({ "message": error }))?;
                    return Err(error);
                }
            }
        }

        let err = "max steps exceeded".to_string();
        self.recorder
            .record("run_error", json!({ "message": err }))?;
        Err(err)
    }
}
