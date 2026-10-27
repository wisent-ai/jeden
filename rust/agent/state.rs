use super::*;

fn mode_state_path(cwd: &Path) -> PathBuf {
    crate::slash::state::mode_state_path(cwd)
}

/// The saved mode state as JSON, refused naming the file when it cannot be
/// read, does not parse, or is not an object; read as empty, every mode was
/// off and the next write replaced the file.
pub(super) fn read_mode_state(cwd: &Path) -> Result<Value, String> {
    crate::slash::state::read_mode_state_value(cwd)
}

/// The mode state's fields to change. `read_mode_state` refuses a state that
/// is not an object, so this refusal names a state replaced in between.
pub(super) fn mode_state_fields(state: &mut Value) -> Result<&mut serde_json::Map<String, Value>, String> {
    state
        .as_object_mut()
        .ok_or_else(|| "the mode state is not a JSON object; nothing was changed".to_string())
}

pub(super) fn write_mode_state(cwd: &Path, state: &Value) -> Result<(), String> {
    let path = mode_state_path(cwd);
    if let Some(parent) = path.parent() {
        crate::slash::state::ensure_mode_state_dir(parent).map_err(|error| {
            format!("cannot create the directory of {}: {error}", path.display())
        })?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|error| {
        format!(
            "cannot encode the mode state for {}: {error}",
            path.display()
        )
    })?;
    fs::write(&path, text + "\n")
        .map_err(|error| format!("cannot write the mode state {}: {error}", path.display()))
}

/// The session `--resume-session` names: a session id, as `jeden sessions`
/// lists it, or its directory.
pub(crate) fn resumed_session(args: &Args) -> Option<PathBuf> {
    args.resume_session
        .as_ref()
        .map(|named| crate::cli::sessions::session_dir_for(&named.to_string_lossy()))
}

/// The session a one-shot session command (`jeden run "/rewind <id>"`) acts
/// on: the one `--resume-session` names, or else the workspace's last
/// durable session.
pub(super) fn command_session(args: &Args) -> Result<PathBuf, String> {
    if let Some(named) = resumed_session(args) {
        return Ok(named);
    }
    let Some(last) = read_mode_state(&args.cwd)?
        .pointer("/lastSessionPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
    else {
        return Err("No prior session found. Run a task first, or name one with --resume-session.".into());
    };
    // A recorded last session that is gone, or cannot be examined, is said
    // as that: both read as "no prior session" and hid where it had been.
    if !crate::tool_runtime::runtime_ops::platform::presence::present(&last)? {
        return Err(format!(
            "The last session {} is gone. Run a task first, or name one with --resume-session.",
            last.display()
        ));
    }
    Ok(last)
}

pub(super) fn apply_mode_instructions(cwd: &Path, task: &str) -> Result<String, String> {
    let state = read_mode_state(cwd)?;
    let mut parts = Vec::new();
    // /force: one-shot forced tool for the next turn, then cleared.
    if let Some(tool) = state
        .pointer("/force/tool")
        .and_then(Value::as_str)
        .filter(|tool| !tool.is_empty())
    {
        parts.push(format!("Forced tool request for this turn: use tool \"{}\" first if it is applicable and available. If it is unsafe or inapplicable, explain why before using another tool.", tool));
        let mut cleared = state.clone();
        if let Some(map) = cleared.as_object_mut() {
            map.insert("force".into(), Value::Null);
            write_mode_state(cwd, &cleared)?;
        }
    }
    // /plan: research + plan, no file changes.
    if state.pointer("/plan/enabled").and_then(Value::as_bool) == Some(true) {
        parts.push("Plan mode is active: research and lay out a concrete, ordered plan for this task before doing the work. Do not modify files unless the user explicitly asks in this turn; end with the plan.".to_string());
    }
    // /goal: keep every step aligned with the stored objective.
    if state.pointer("/goal/enabled").and_then(Value::as_bool) == Some(true) {
        if let Some(objective) = state
            .pointer("/goal/objective")
            .and_then(Value::as_str)
            .filter(|o| !o.trim().is_empty())
        {
            let budget = match state.pointer("/goal/budget").and_then(Value::as_f64) {
                Some(budget) => format!(" Respect the working budget of {}.", budget),
                None => String::new(),
            };
            parts.push(format!("Active goal: {}. Keep every step aligned with this goal and note progress toward it.{}", objective.trim(), budget));
        }
    }
    // /guided-goal: one-shot — refine a rough objective this turn, then clear it.
    if state.pointer("/guidedGoal/active").and_then(Value::as_bool) == Some(true) {
        if let Some(rough) = state
            .pointer("/guidedGoal/roughObjective")
            .and_then(Value::as_str)
            .filter(|o| !o.trim().is_empty())
        {
            parts.push(format!("Guided goal drafting: the user's rough objective is \"{}\". Before doing the work, restate it as a concrete, measurable goal with clear success criteria, then proceed toward it.", rough.trim()));
        }
        let mut cleared = state.clone();
        if let Some(map) = cleared.as_object_mut() {
            map.insert(
                "guidedGoal".into(),
                json!({ "active": false, "roughObjective": "" }),
            );
            write_mode_state(cwd, &cleared)?;
        }
    }
    // /shake: distrust heavy prior context unless re-read.
    if let Some(shake) = state
        .get("shake")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        parts.push(format!("Shake mode ({}): do not rely on heavy prior context or artifacts unless you re-read them this turn; re-verify assumptions from source.", shake.trim()));
    }
    if parts.is_empty() {
        Ok(task.to_string())
    } else {
        Ok(format!("{}\n\n{}", parts.join("\n"), task))
    }
}

/// When plan mode is enabled, store `text` as the latest plan so `/plan-review`
/// can surface it. A mode state that cannot be read is the caller's error: it
/// is never taken for plan mode being off.
pub(super) fn capture_plan_if_enabled(cwd: &Path, text: &str) -> Result<(), String> {
    let mut state = read_mode_state(cwd)?;
    if state.pointer("/plan/enabled").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    if let Some(obj) = state.as_object_mut() {
        let plan = obj.entry("plan").or_insert_with(|| json!({}));
        if let Some(plan_obj) = plan.as_object_mut() {
            plan_obj.insert("latestPlan".into(), json!(text));
        }
        write_mode_state(cwd, &state)?;
    }
    Ok(())
}

/// If loop mode is active and not spent, return the stored prompt to
/// resubmit and consume one iteration (decrement `remaining`; disable when the
/// remaining count is spent or the `until` end time passes). Returns `None`
/// when the loop stops, including when `/loop` stored no explicit prompt. A
/// mode state that cannot be read is the error, not a stopped loop.
pub(crate) fn loop_next_prompt(cwd: &Path, _current_task: &str) -> Result<Option<String>, String> {
    let mut state = read_mode_state(cwd)?;
    if state.pointer("/loop_mode/enabled").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }
    // Duration guard: stop once the end time has passed.
    if let Some(until) = state.pointer("/loop_mode/until").and_then(Value::as_u64) {
        let now = crate::clock::unix_millis()
            .map_err(|error| format!("the /loop end time cannot be checked: {error}"))?;
        if now >= until {
            disable_loop(cwd, &mut state)?;
            return Ok(None);
        }
    }
    // Count guard: a count with no iteration left to take stops the loop;
    // otherwise this iteration is taken from it.
    let remaining = match state
        .pointer("/loop_mode/remaining")
        .and_then(Value::as_u64)
    {
        None => None,
        Some(count) => match count.checked_sub(u64::from(crate::tally::STEP)) {
            Some(next) => Some(next),
            None => {
                disable_loop(cwd, &mut state)?;
                return Ok(None);
            }
        },
    };
    let Some(prompt) = state
        .pointer("/loop_mode/prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string)
    else {
        disable_loop(cwd, &mut state)?;
        return Ok(None);
    };
    if let Some(next) = remaining {
        if let Some(obj) = state.as_object_mut() {
            if let Some(lm) = obj.get_mut("loop_mode").and_then(Value::as_object_mut) {
                lm.insert("remaining".into(), json!(next));
            }
        }
        // An iteration that cannot be counted is not run: unrecorded, the
        // same iteration would be resubmitted for ever.
        write_mode_state(cwd, &state)
            .map_err(|error| format!("loop mode could not count this iteration: {error}"))?;
    }
    Ok(Some(prompt))
}

/// Turn loop mode off in the saved mode state; a state that cannot be written
/// is the error, since a loop still enabled there would run again.
fn disable_loop(cwd: &Path, state: &mut Value) -> Result<(), String> {
    if let Some(obj) = state.as_object_mut() {
        obj.insert("loop_mode".into(), json!({ "enabled": false }));
    }
    write_mode_state(cwd, state)
        .map_err(|error| format!("loop mode could not be turned off: {error}"))
}

pub(crate) fn update_task_outcome(cwd: &Path, task: &str, ok: bool) -> Result<(), String> {
    let mut state = read_mode_state(cwd)?;
    let map = mode_state_fields(&mut state)?;
    if ok {
        map.insert("lastTask".into(), json!(task));
        map.insert("lastFailedTask".into(), json!(""));
    } else {
        map.insert("lastFailedTask".into(), json!(task));
    }
    write_mode_state(cwd, &state)
}

pub(crate) fn update_last_session_path(cwd: &Path, path: &Path) -> Result<(), String> {
    let mut state = read_mode_state(cwd)?;
    mode_state_fields(&mut state)?.insert("lastSessionPath".into(), json!(path));
    write_mode_state(cwd, &state)
}

/// Append a branch record to mode-state so `/tree` can list it and `/resume`
/// can navigate to its session path. Backs a real fork-based `/branch`. The
/// branch is named by its session, which no other branch shares.
pub(crate) fn record_branch(cwd: &Path, title: &str, path: &Path) -> Result<String, String> {
    let id = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            format!(
                "a branch cannot be named: {} has no session name",
                path.display()
            )
        })?;
    let created_at = now_stamp()?;
    let mut state = read_mode_state(cwd)?;
    let map = mode_state_fields(&mut state)?;
    let roadmap_item = map
        .get("activeRoadmapItem")
        .and_then(Value::as_str)
        .map(str::to_string);
    let branches = map.entry("branches").or_insert_with(|| json!([]));
    let arr = branches.as_array_mut().ok_or("branches is not an array")?;
    let title = if title.trim().is_empty() {
        id.clone()
    } else {
        title.trim().to_string()
    };
    arr.push(json!({
        "id": id,
        "title": title,
        "createdAt": created_at,
        "path": path.to_string_lossy(),
        "roadmapItem": roadmap_item
    }));
    write_mode_state(cwd, &state)?;
    Ok(id)
}
