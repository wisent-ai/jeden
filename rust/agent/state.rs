use super::*;

fn mode_state_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/mode-state.json")
}

pub(super) fn read_mode_state(cwd: &Path) -> Value {
    fs::read_to_string(mode_state_path(cwd))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

pub(super) fn write_mode_state(cwd: &Path, state: &Value) -> Result<(), String> {
    let path = mode_state_path(cwd);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())
}

pub(super) fn apply_mode_instructions(cwd: &Path, task: &str) -> Result<String, String> {
    let state = read_mode_state(cwd);
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
    if state
        .pointer("/plan/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        parts.push("Plan mode is active: research and lay out a concrete, ordered plan for this task before doing the work. Do not modify files unless the user explicitly asks in this turn; end with the plan.".to_string());
    }
    // /goal: keep every step aligned with the stored objective.
    if state
        .pointer("/goal/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if let Some(objective) = state
            .pointer("/goal/objective")
            .and_then(Value::as_str)
            .filter(|o| !o.trim().is_empty())
        {
            let budget = state
                .pointer("/goal/budget")
                .and_then(Value::as_f64)
                .map(|b| format!(" Respect the working budget of {}.", b))
                .unwrap_or_default();
            parts.push(format!("Active goal: {}. Keep every step aligned with this goal and note progress toward it.{}", objective.trim(), budget));
        }
    }
    // /guided-goal: one-shot — refine a rough objective this turn, then clear it.
    if state
        .pointer("/guidedGoal/active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
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
/// can surface it. Best-effort; a write failure never fails the turn.
pub(super) fn capture_plan_if_enabled(cwd: &Path, text: &str) {
    let mut state = read_mode_state(cwd);
    if !state
        .pointer("/plan/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    if let Some(obj) = state.as_object_mut() {
        let plan = obj.entry("plan").or_insert_with(|| json!({}));
        if let Some(plan_obj) = plan.as_object_mut() {
            plan_obj.insert("latestPlan".into(), json!(text));
        }
        let _ = write_mode_state(cwd, &state);
    }
}

/// Hard ceiling on loop-mode auto-resubmissions in a single invocation, so an
/// unbounded `/loop` can never spin forever.
pub(crate) const MAX_LOOP_ITERS: u32 = 50;

fn now_millis_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// If loop mode is active and not exhausted, return the stored prompt to
/// resubmit and consume one iteration (decrement `remaining`; disable when the
/// remaining count is exhausted or the `until` deadline passes). Returns `None`
/// when the loop stops, including when `/loop` stored no explicit prompt.
pub(crate) fn loop_next_prompt(cwd: &Path, _current_task: &str) -> Option<String> {
    let mut state = read_mode_state(cwd);
    if !state
        .pointer("/loop_mode/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    // Duration guard: stop once the deadline has passed.
    if let Some(until) = state.pointer("/loop_mode/until").and_then(Value::as_u64) {
        if now_millis_u64() >= until {
            disable_loop(cwd, &mut state);
            return None;
        }
    }
    // Count guard: a remaining of 0 means exhausted.
    let remaining = state
        .pointer("/loop_mode/remaining")
        .and_then(Value::as_u64);
    if remaining == Some(0) {
        disable_loop(cwd, &mut state);
        return None;
    }
    let prompt = state
        .pointer("/loop_mode/prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if prompt.trim().is_empty() {
        disable_loop(cwd, &mut state);
        return None;
    }
    // Consume one iteration.
    if let Some(n) = remaining {
        if let Some(obj) = state.as_object_mut() {
            if let Some(lm) = obj.get_mut("loop_mode").and_then(Value::as_object_mut) {
                let next = n.saturating_sub(1);
                lm.insert("remaining".into(), json!(next));
            }
        }
        let _ = write_mode_state(cwd, &state);
    }
    Some(prompt)
}

fn disable_loop(cwd: &Path, state: &mut Value) {
    if let Some(obj) = state.as_object_mut() {
        obj.insert("loop_mode".into(), json!({ "enabled": false }));
    }
    let _ = write_mode_state(cwd, state);
}

pub(crate) fn update_task_outcome(cwd: &Path, task: &str, ok: bool) -> Result<(), String> {
    let mut state = read_mode_state(cwd);
    if !state.is_object() {
        state = json!({});
    }
    let map = state.as_object_mut().expect("mode state object");
    if ok {
        map.insert("lastTask".into(), json!(task));
        map.insert("lastFailedTask".into(), json!(""));
    } else {
        map.insert("lastFailedTask".into(), json!(task));
    }
    write_mode_state(cwd, &state)
}

pub(crate) fn update_last_session_path(cwd: &Path, path: &Path) -> Result<(), String> {
    let mut state = read_mode_state(cwd);
    if !state.is_object() {
        state = json!({});
    }
    state
        .as_object_mut()
        .expect("mode state object")
        .insert("lastSessionPath".into(), json!(path));
    write_mode_state(cwd, &state)
}

/// Append a branch record to mode-state so `/tree` can list it and `/resume`
/// can navigate to its session path. Backs a real fork-based `/branch`.
pub(crate) fn record_branch(cwd: &Path, title: &str, path: &Path) -> Result<String, String> {
    let mut state = read_mode_state(cwd);
    if !state.is_object() {
        state = json!({});
    }
    let map = state.as_object_mut().expect("mode state object");
    let roadmap_item = map
        .get("activeRoadmapItem")
        .and_then(Value::as_str)
        .map(str::to_string);
    let branches = map.entry("branches").or_insert_with(|| json!([]));
    let arr = branches.as_array_mut().ok_or("branches is not an array")?;
    let id = format!("branch-{}", arr.len() + 1);
    let title = if title.trim().is_empty() {
        id.clone()
    } else {
        title.trim().to_string()
    };
    arr.push(json!({
        "id": id,
        "title": title,
        "createdAt": now_stamp(),
        "path": path.to_string_lossy(),
        "roadmapItem": roadmap_item
    }));
    write_mode_state(cwd, &state)?;
    Ok(id)
}
