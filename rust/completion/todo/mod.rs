use super::{model::*, operations::snapshot_value, store};
use serde_json::{json, Value};
use std::path::Path;

fn text<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input.get(field).and_then(Value::as_str).filter(|text| !text.trim().is_empty())
}

fn item_text(value: &Value) -> Result<&str, String> {
    value.as_str().or_else(|| text(value, "text"))
        .or_else(|| text(value, "task")).or_else(|| text(value, "name"))
        .filter(|text| !text.trim().is_empty()).ok_or_else(|| "todo item needs nonempty text".into())
}

fn append_items(state: &mut CompletionState, phase: &str, items: &[Value]) -> Result<(), String> {
    let request = state.requests.iter().rev().find(|request| !request.coverage_verified && !request.paused)
        .ok_or("todo requires an active captured user request")?;
    let request_id = request.id.clone();
    let kind = state.tasks.iter().find(|task| task.request_id == request_id)
        .map(|task| task.kind).unwrap_or(TaskKind::Work);
    for item in items {
        let title = item_text(item)?;
        if state.tasks.iter().any(|task| task.text == title && !task.status.terminal()) {
            continue;
        }
        state.tasks.push(WorkTask {
            id: uuid::Uuid::new_v4().to_string(),
            request_id: request_id.clone(),
            phase: phase.to_string(),
            text: title.to_string(),
            criteria: vec![title.to_string()],
            kind,
            origin: TaskOrigin::Agent,
            status: TaskStatus::Pending,
            reason: None,
            verification: None,
        });
    }
    Ok(())
}

fn selected(state: &CompletionState, input: &Value, allow_all: bool) -> Result<Vec<usize>, String> {
    if let Some(selector) = text(input, "task") {
        let matches: Vec<_> = state.tasks.iter().enumerate()
            .filter(|(_, task)| task.id == selector || task.text == selector)
            .map(|(index, _)| index).collect();
        if matches.is_empty() {
            return Err(format!("unknown task: {selector}"));
        }
        if matches.len() != usize::from(true) {
            return Err(format!("task text is ambiguous; use its task id: {selector}"));
        }
        return Ok(matches);
    }
    if let Some(phase) = text(input, "phase") {
        let matches: Vec<_> = state.tasks.iter().enumerate()
            .filter(|(_, task)| task.phase == phase).map(|(index, _)| index).collect();
        if matches.is_empty() {
            return Err(format!("unknown phase: {phase}"));
        }
        return Ok(matches);
    }
    if allow_all {
        return Ok(state.tasks.iter().enumerate().map(|(index, _)| index).collect());
    }
    Err("todo requires a task id or phase".into())
}

pub(crate) fn execute(session: &Path, input: &Value) -> Result<Value, String> {
    let op = text(input, "op").unwrap_or("view");
    if op == "view" {
        return Ok(summary(&store::read(session)?));
    }
    let (_, state) = store::update(session, None, |state| {
        match op {
            "init" | "append" => {
                // Initializing a plan is additive. It cannot erase prior user work.
                if let Some(phases) = input.get("list").and_then(Value::as_array) {
                    if phases.is_empty() {
                        return Err("todo init requires at least one phase".into());
                    }
                    for phase in phases {
                        let name = text(phase, "phase").or_else(|| text(phase, "name")).unwrap_or("Tasks");
                        let items = phase.get("items").and_then(Value::as_array).ok_or("todo phase requires items")?;
                        append_items(state, name, items)?;
                    }
                } else {
                    let items = input.get("items").and_then(Value::as_array).ok_or("todo requires items")?;
                    append_items(state, text(input, "phase").unwrap_or("Tasks"), items)?;
                }
            }
            "start" | "done" => {
                let indexes = selected(state, input, false)?;
                if indexes.iter().any(|index| state.tasks[*index].status == TaskStatus::Paused) {
                    return Err("task was paused by the operator; the agent cannot resume it".into());
                }
                if op == "start" {
                    for task in &mut state.tasks {
                        if task.status == TaskStatus::InProgress {
                            task.status = TaskStatus::Pending;
                        }
                    }
                }
                for index in indexes {
                    let task = &mut state.tasks[index];
                    if task.status.terminal() {
                        continue;
                    }
                    task.status = if op == "start" {
                        TaskStatus::InProgress
                    } else {
                        TaskStatus::VerificationRequested
                    };
                    task.reason = if op == "done" {
                        Some("The agent requested verification; completion has not been accepted.".into())
                    } else {
                        None
                    };
                    task.verification = None;
                }
            }
            "drop" | "rm" => {
                let indexes = selected(state, input, op == "rm")?;
                if indexes.iter().any(|index| state.tasks[*index].origin == TaskOrigin::User) {
                    return Err("the agent cannot remove or cancel user tasks; only an explicit operator action can withdraw them".into());
                }
                for index in indexes {
                    let task = &mut state.tasks[index];
                    if !task.status.terminal() {
                        task.status = TaskStatus::Cancelled;
                        task.reason = Some("Agent withdrew its own planning item; user requirements remain unchanged.".into());
                    }
                }
            }
            _ => return Err(format!("unknown todo op: {op}")),
        }
        Ok(())
    })?;
    Ok(summary(&state))
}

fn summary(state: &CompletionState) -> Value {
    let mut phases: Vec<Value> = Vec::new();
    for task in &state.tasks {
        let index = phases.iter().position(|phase| phase["phase"] == task.phase)
            .unwrap_or_else(|| {
                phases.push(json!({"phase": task.phase, "items": []}));
                phases.len() - usize::from(true)
            });
        phases[index]["items"].as_array_mut().expect("phase items")
            .push(serde_json::to_value(task).expect("task serializes"));
    }
    let mut output = snapshot_value(state);
    output["phases"] = json!(phases);
    output["items"] = json!(state.tasks);
    output["active"] = state.tasks.iter()
        .find(|task| task.status == TaskStatus::InProgress)
        .or_else(|| state.tasks.iter().find(|task| !task.status.terminal()))
        .map(|task| json!(task.text)).unwrap_or(Value::Null);
    output
}
