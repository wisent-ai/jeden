use super::{model::*, operations::snapshot_value, store};
use serde_json::{json, Value};
use std::path::Path;

fn text<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

fn item_text(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .or_else(|| text(value, "text"))
        .or_else(|| text(value, "task"))
        .or_else(|| text(value, "name"))
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "todo item needs nonempty text".into())
}

fn append_items(state: &mut CompletionState, phase: &str, items: &[Value]) -> Result<(), String> {
    let request = state
        .requests
        .iter()
        .rev()
        .find(|request| !request.coverage_verified && !request.paused)
        .ok_or("todo requires an active captured user request")?;
    let request_id = request.id.clone();
    // Items added to a request that is answered rather than worked are
    // answer items too; every other request's items are work.
    let kind = match state
        .tasks
        .iter()
        .find(|task| task.request_id == request_id)
    {
        Some(task) if task.kind == TaskKind::Answer => TaskKind::Answer,
        _ => TaskKind::Work,
    };
    for item in items {
        let title = item_text(item)?;
        if state
            .tasks
            .iter()
            .any(|task| task.text == title && !task.status.terminal())
        {
            continue;
        }
        state.tasks.push(WorkTask {
            id: uuid::Uuid::new_v4().to_string(),
            request_id: request_id.clone(),
            phase: phase.to_string(),
            text: title.to_string(),
            criteria: vec![title.to_string()],
            kind,
            defect_of: None,
            origin: TaskOrigin::Agent,
            status: TaskStatus::Pending,
            reason: None,
            verification: None,
            operator_request: None,
        });
    }
    Ok(())
}

fn selected(state: &CompletionState, input: &Value, allow_all: bool) -> Result<Vec<usize>, String> {
    if let Some(selector) = text(input, "task") {
        let matches: Vec<_> = state
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| task.id == selector || task.text == selector)
            .map(|(index, _)| index)
            .collect();
        match matches.as_slice() {
            [] => return Err(format!("unknown task: {selector}")),
            [_] => {}
            _ => {
                return Err(format!(
                    "task text is ambiguous; use its task id: {selector}"
                ))
            }
        }
        return Ok(matches);
    }
    if let Some(phase) = text(input, "phase") {
        let matches: Vec<_> = state
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| task.phase == phase)
            .map(|(index, _)| index)
            .collect();
        if matches.is_empty() {
            return Err(format!("unknown phase: {phase}"));
        }
        return Ok(matches);
    }
    if allow_all {
        return Ok(state
            .tasks
            .iter()
            .enumerate()
            .map(|(index, _)| index)
            .collect());
    }
    Err("todo requires a task id or phase".into())
}

/// What one call of the todo tool does, spelled by its serde name in the
/// tool's `op` field; the tool schema lists the same names.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TodoOp {
    /// Start a plan; additive, like `append`.
    Init,
    Append,
    Start,
    /// Request verification; never marks work complete.
    Done,
    /// Withdraw the selected agent items.
    Drop,
    /// Withdraw agent items, every one when nothing is selected.
    Rm,
    View,
}

impl TodoOp {
    const ALL: &'static [TodoOp] = &[
        TodoOp::Init,
        TodoOp::Append,
        TodoOp::Start,
        TodoOp::Done,
        TodoOp::Drop,
        TodoOp::Rm,
        TodoOp::View,
    ];

    /// Every op's name, for the tool schema and refusals.
    pub(crate) fn names() -> Vec<String> {
        Self::ALL
            .iter()
            .map(|op| crate::vocabulary::spelling(*op))
            .collect()
    }

    fn of(input: &Value) -> Result<Self, String> {
        let named = text(input, "op")
            .ok_or_else(|| format!("todo requires op, one of {}", Self::names().join(", ")))?;
        crate::vocabulary::named(named).map_err(|cause| {
            format!(
                "unknown todo op {named:?} ({cause}); the ops are {}",
                Self::names().join(", ")
            )
        })
    }
}

/// The phase a group of items joins, which the caller names.
fn phase_name(value: &Value) -> Result<&str, String> {
    text(value, "phase")
        .ok_or_else(|| "todo items need phase: the name of the phase they join".into())
}

/// Add the items `input` carries, phased or not. Adding is all a plan does:
/// it cannot erase prior user work.
fn add(state: &mut CompletionState, input: &Value) -> Result<(), String> {
    let Some(phases) = input.get("list").and_then(Value::as_array) else {
        let items = input
            .get("items")
            .and_then(Value::as_array)
            .ok_or("todo requires items")?;
        return append_items(state, phase_name(input)?, items);
    };
    if phases.is_empty() {
        return Err("todo init requires at least one phase".into());
    }
    for phase in phases {
        let items = phase
            .get("items")
            .and_then(Value::as_array)
            .ok_or("todo phase requires items")?;
        append_items(state, phase_name(phase)?, items)?;
    }
    Ok(())
}

/// Move the selected tasks to `status`, with `reason`; `exclusive` first
/// returns the task in progress to pending, so one task is worked at a time.
fn advance(
    state: &mut CompletionState,
    input: &Value,
    status: TaskStatus,
    reason: Option<&str>,
    exclusive: bool,
) -> Result<(), String> {
    let indexes = selected(state, input, false)?;
    if indexes
        .iter()
        .any(|index| state.tasks[*index].status == TaskStatus::Paused)
    {
        return Err("task was paused by the operator; the agent cannot resume it".into());
    }
    if exclusive {
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
        task.status = status;
        task.reason = reason.map(str::to_string);
        task.verification = None;
    }
    Ok(())
}

/// Cancel the selected agent items; `all` selects every item when the call
/// names none. User tasks are never withdrawn by the agent.
fn withdraw(state: &mut CompletionState, input: &Value, all: bool) -> Result<(), String> {
    let indexes = selected(state, input, all)?;
    if indexes
        .iter()
        .any(|index| state.tasks[*index].origin == TaskOrigin::User)
    {
        return Err("the agent cannot remove or cancel user tasks; only an explicit operator action can withdraw them".into());
    }
    for index in indexes {
        let task = &mut state.tasks[index];
        if !task.status.terminal() {
            task.status = TaskStatus::Cancelled;
            task.reason = Some(
                "Agent withdrew its own planning item; user requirements remain unchanged.".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn execute(session: &Path, input: &Value) -> Result<Value, String> {
    let op = TodoOp::of(input)?;
    if op == TodoOp::View {
        return Ok(summary(&store::read(session)?));
    }
    let (_, state) = store::update(session, None, |state| match op {
        TodoOp::View => Ok(()),
        TodoOp::Init => add(state, input),
        TodoOp::Append => add(state, input),
        TodoOp::Start => advance(state, input, TaskStatus::InProgress, None, true),
        TodoOp::Done => advance(
            state,
            input,
            TaskStatus::VerificationRequested,
            Some("The agent requested verification; completion has not been accepted."),
            false,
        ),
        TodoOp::Drop => withdraw(state, input, false),
        TodoOp::Rm => withdraw(state, input, true),
    })?;
    Ok(summary(&state))
}

fn summary(state: &CompletionState) -> Value {
    let mut phases: Vec<Value> = Vec::new();
    for task in &state.tasks {
        let index = match phases.iter().position(|phase| phase["phase"] == task.phase) {
            Some(index) => index,
            None => {
                let index = phases.len();
                phases.push(json!({"phase": task.phase, "items": []}));
                index
            }
        };
        phases[index]["items"]
            .as_array_mut()
            .expect("phase items")
            .push(serde_json::to_value(task).expect("task serializes"));
    }
    let mut output = snapshot_value(state);
    output["phases"] = json!(phases);
    output["items"] = json!(state.tasks);
    // The active item: the first in progress, else the first not finished,
    // else none.
    let active = state
        .tasks
        .iter()
        .filter(|task| !task.status.terminal())
        .min_by_key(|task| task.status != TaskStatus::InProgress);
    output["active"] = match active {
        Some(task) => json!(task.text),
        None => Value::Null,
    };
    output
}
