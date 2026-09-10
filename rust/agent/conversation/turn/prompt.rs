//! Everything one turn assembles before its first model call: the effective
//! prompt, the model route, the contract text, and the durable record of the
//! request that arrived.

use super::super::*;

/// What the prologue decided, held for the rest of the turn.
pub(super) struct Prepared {
    pub(super) config: Config,
    pub(super) router: crate::model_router::ChatConfig,
    pub(super) report_required: bool,
    /// Whether this conversation owns the user's retained work, which decides
    /// whether a refusal becomes a recorded blocker or only this turn's error.
    pub(super) tracks_completion: bool,
    pub(super) language: crate::cli::config::UiLanguage,
    pub(super) classification: Option<std::thread::JoinHandle<()>>,
}

impl Conversation {
    pub(super) fn prepare_turn(
        &mut self,
        args: &Args,
        task: &str,
        continuing: bool,
        tracks_completion: bool,
        completion_request: Option<String>,
        hooks: &RunHooks<'_>,
    ) -> Result<Prepared, String> {
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        // The stage that can change the product owes the delivery report,
        // whoever drove it. Excluding every autonomous stage exempted the
        // Pursuit execution stage — the one that writes code — so it answered
        // with prose, recorded no `task_report`, and nothing checked whether
        // it had covered the CLI, the GUI, the documentation or real tests.
        // Read-only stages (planning, reviews) still keep their own output
        // contracts, and they are read-only precisely because they may not
        // write.
        let report_required =
            !args.model_only && (!args.autonomous || args.allow_write || args.allow_command);
        let language = crate::cli::config::ui_language(&config);
        let mut effective_task = if args.model_only || args.autonomous {
            task.to_string()
        } else {
            apply_mode_instructions(&args.cwd, task)?
        };
        if let Some(goal) = args
            .goal
            .as_deref()
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
        {
            effective_task = format!(
                "Active work goal for this turn: {}. Keep every step aligned with this goal and report completed work against it.\n\n{}",
                goal, effective_task
            );
        }
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
        if report_required {
            effective_task.push_str("\n\n");
            effective_task.push_str(task_contract::turn_instruction());
        }
        self.recorder.record(
            if continuing { "auto_continue" } else { "user" },
            json!({
                "task": effective_task,
                "rawTask": task,
                "prompt": effective_task,
                "completionRequestId": completion_request,
                "cwd": args.cwd,
                "allowWrite": args.allow_write,
                "allowCommand": args.allow_command,
                "maxSteps": args.max_steps,
                "maxTokens": args.max_tokens,
                "modelOnly": args.model_only,
                "completionManaged": tracks_completion,
                "goal": args.goal,
            }),
        )?;
        if report_required {
            let mut contract = task_contract::snapshot(&language);
            contract["task"] = json!(task);
            self.recorder.record("task_contract", contract)?;
        }
        if tracks_completion {
            if continuing {
                crate::completion::clear_runtime_blocker(&self.recorder.path())?;
            }
            self.prepare_completion(args, hooks)?;
        }
        let mut classification = None;
        if !args.model_only && !args.autonomous && !continuing {
            // Oko goal-lifecycle classification: background-only and fail-open.
            // Results update mode state and session events, never this turn's
            // prompt text. The ledger event lands via the process-wide append
            // lock, so it is safe next to this thread's own recording.
            let turn_index = self
                .messages
                .iter()
                .filter(|message| {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("user")
                })
                .count() as u64;
            classification = Some(crate::goal_lifecycle::spawn_turn_classification(
                args.cwd.clone(),
                task.to_string(),
                self.recorder.path(),
                turn_index,
                hooks.goal_event.clone(),
            ));
        }
        self.messages
            .push(json!({ "role": "user", "content": effective_task }));
        Ok(Prepared {
            config,
            router,
            report_required,
            tracks_completion,
            language,
            classification,
        })
    }
}
