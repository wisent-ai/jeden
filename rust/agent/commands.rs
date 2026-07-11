use super::*;

fn split_head(args: &str) -> (&str, &str) {
    let text = args.trim();
    if text.is_empty() {
        return ("", "");
    }
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

/// Resolve the prompt text a `/retry` should replay, or an error. Lets the
/// interactive loop replay through the shared Conversation instead of a
/// transient one.
pub(crate) fn retry_task(args: &Args) -> Result<String, String> {
    let state = read_mode_state(&args.cwd);
    let task = state
        .get("lastFailedTask")
        .and_then(Value::as_str)
        .filter(|task| !task.trim().is_empty())
        .ok_or("No failed task is available to retry.")?;
    if task.trim_start().starts_with('/') {
        return Err("Refusing to retry a slash command; retry only replays agent prompts.".into());
    }
    Ok(task.to_string())
}

/// Build the scoped prompt text for a `/btw` side question.
pub(crate) fn btw_task(question: &str) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("Usage: /btw <side question>".into());
    }
    Ok(format!(
        "Answer this side question using the current session context.\nKeep it separate from the main task: do not change files unless the side question explicitly asks for file changes.\nQuestion: {}",
        question
    ))
}

pub(crate) fn retry_command_with(args: &Args, hooks: &mut RunHooks) -> Result<String, String> {
    let state = read_mode_state(&args.cwd);
    let task = state
        .get("lastFailedTask")
        .and_then(Value::as_str)
        .filter(|task| !task.trim().is_empty())
        .ok_or("No failed task is available to retry.")?;
    if task.trim_start().starts_with('/') {
        return Err("Refusing to retry a slash command; retry only replays agent prompts.".into());
    }
    let mut retry_args = args.clone();
    retry_args.command = "run".into();
    retry_args.positionals = vec![task.to_string()];
    run_command_with(&retry_args, hooks)
}

pub(crate) fn btw_command_with(
    args: &Args,
    question: &str,
    hooks: &mut RunHooks,
) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("Usage: /btw <side question>".into());
    }
    let mut side_args = args.clone();
    side_args.command = "run".into();
    side_args.positionals = vec![format!(
        "Answer this side question using the current session context.\nKeep it separate from the main task: do not change files unless the side question explicitly asks for file changes.\nQuestion: {}",
        question
    )];
    run_command_with(&side_args, hooks)
}

pub(crate) fn run_command(args: &Args) -> Result<String, String> {
    run_command_with(args, &mut RunHooks::inert())
}

/// Run a session-scoped slash (`/compact`, `/handoff`, `/context`) in the
/// one-shot CLI by loading the last session's history into a Conversation and
/// invoking the same real methods the interactive path uses — no divergent stub.
fn run_session_command(
    args: &Args,
    command: &str,
    rest: &str,
    hooks: &mut RunHooks,
) -> Result<String, String> {
    let last = read_mode_state(&args.cwd)
        .pointer("/lastSessionPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .ok_or("No prior session found. Run a task first, then /compact, /handoff, or /context.")?;
    let turns = crate::session_conversation_turns(&last)?;
    let mut conversation = Conversation::new(&args.cwd)?;
    conversation.load_history(&args.cwd, turns)?;
    match command {
        "/compact" => conversation.compact(args, rest, hooks),
        "/handoff" => conversation.handoff(args, rest, hooks),
        "/context" => Ok(format!(
            "Loaded conversation: {} message(s), ~{} tokens (from {}).",
            conversation.turn_len(),
            conversation.approx_tokens(),
            last.display()
        )),
        _ => unreachable!(),
    }
}

/// Validate `tool` against the visible tool set and write it to mode-state so
/// the next turn's apply_mode_instructions injects (and clears) the force
/// directive. Backs the combined `/force <tool> <prompt>` form.
pub(crate) fn arm_force_tool(cwd: &Path, tool: &str) -> Result<(), String> {
    let names: Vec<String> = crate::tools::list_tools(cwd)
        .into_iter()
        .map(|t| t.name)
        .collect();
    if !names.is_empty() && !names.iter().any(|n| n == tool) {
        return Err(format!(
            "Unknown or unavailable tool: {}. Visible tools: {}",
            tool,
            names
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut state = read_mode_state(cwd);
    if !state.is_object() {
        state = json!({});
    }
    state
        .as_object_mut()
        .expect("mode state object")
        .insert("force".into(), json!({ "tool": tool, "prompt": "" }));
    write_mode_state(cwd, &state)
}

pub(crate) fn run_command_with(args: &Args, hooks: &mut RunHooks) -> Result<String, String> {
    let mut task = args.positionals.join(" ").trim().to_string();
    if task.trim_start().starts_with('/') {
        let (command, rest) = split_head(task.trim());
        let (command, rest) = (command.to_string(), rest.to_string());
        if command == "/retry" {
            return retry_command_with(args, hooks);
        }
        if command == "/btw" {
            return btw_command_with(args, &rest, hooks);
        }
        // /compact, /handoff, /context operate on the last session's history so
        // the one-shot CLI matches the interactive implementations (no divergent
        // flag-set / markdown-dump / tool-manifest stubs).
        if matches!(command.as_str(), "/compact" | "/handoff" | "/context") {
            let text = run_session_command(args, &command, &rest, hooks)?;
            return Ok(if args.json {
                json!({ "text": text }).to_string() + "\n"
            } else {
                text + "\n"
            });
        }
        // /force <tool> <prompt>: arm the forced tool, then run the prompt now
        // (apply_mode_instructions injects the directive and clears it). The bare
        // /force <tool> form falls through to handle_slash (deferred).
        if command == "/force" {
            let (tool, prompt) = split_head(&rest);
            if !tool.is_empty() && !prompt.trim().is_empty() {
                arm_force_tool(&args.cwd, tool)?;
                task = prompt.trim().to_string();
                // fall through to run the turn below with force armed.
            } else {
                let text = handle_slash(&args.cwd, task.trim(), args.model.as_deref())?;
                return Ok(if args.json {
                    json!({ "text": text }).to_string() + "\n"
                } else {
                    text + "\n"
                });
            }
        } else if crate::is_builtin_slash(&command) {
            // Builtins are handled locally; file-based custom commands expand to a
            // prompt; unknown slash input forwards to the model literally.
            let text = handle_slash(&args.cwd, task.trim(), args.model.as_deref())?;
            return Ok(if args.json {
                json!({ "text": text }).to_string() + "\n"
            } else {
                text + "\n"
            });
        } else if let Some(expanded) = crate::resolve_file_command(&args.cwd, &command, &rest) {
            task = expanded;
        }
    }

    let mut conversation = Conversation::new(&args.cwd)?;
    let result = conversation.run_turn(args, &task, hooks);
    if let Err(error) = &result {
        let _ = update_task_outcome(&args.cwd, &task, false);
        return Err(error.clone());
    }
    let mut text = result?;
    let _ = update_task_outcome(&args.cwd, &task, true);
    // Loop mode: auto-resubmit the loop prompt until exhausted (bounded).
    let mut iters = 0;
    while iters < MAX_LOOP_ITERS {
        let Some(loop_prompt) = loop_next_prompt(&args.cwd, &task) else {
            break;
        };
        if hooks.cancelled() {
            break;
        }
        match conversation.run_turn(args, &loop_prompt, hooks) {
            Ok(more) => {
                text = format!("{}\n\n— loop resubmit —\n{}", text, more);
            }
            Err(error) => {
                text = format!("{}\n\n— loop resubmit failed —\n{}", text, error);
                break;
            }
        }
        iters += 1;
    }
    let session_path = Some(conversation.session_path());
    let result = RunResult {
        text,
        session_path: session_path.clone(),
    };
    if let Some(path) = &session_path {
        let _ = update_last_session_path(&args.cwd, path);
    }
    if args.json {
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "repaired": false,
            "originalError": Value::Null,
            "text": result.text,
            "sessionPath": result.session_path,
        }))
        .map_err(|e| e.to_string())?
            + "\n");
    }

    if let Some(path) = &result.session_path {
        eprintln!("[session] {}", path.display());
    }
    Ok(result.text + "\n")
}
