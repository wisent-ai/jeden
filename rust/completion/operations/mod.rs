use super::{model::*, store};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) fn capture_request(session: &Path, cwd: &Path, prompt: &str) -> Result<(String, CompletionState), String> {
    if prompt.trim().is_empty() {
        return Err("completion request must not be empty".into());
    }
    store::update(session, None, |state| {
        let id = uuid::Uuid::new_v4().to_string();
        state.requests.push(WorkRequest {
            id: id.clone(),
            prompt: prompt.to_string(),
            cwd: cwd.display().to_string(),
            paused: false,
            captured_at: crate::agent::now_stamp(),
            planned: false,
            coverage_verified: false,
        });
        state.blocker = None;
        Ok(id)
    })
}

pub(crate) fn plan_request(
    session: &Path,
    revision: u64,
    request_id: &str,
    plan: IntakePlan,
) -> Result<CompletionState, String> {
    let (_, state) = store::update(session, Some(revision), |state| {
        let request = state.requests.iter().find(|request| request.id == request_id)
            .ok_or("completion intake references an unknown request")?;
        if request.planned {
            return Err("acceptance requirements are already recorded for this request".into());
        }
        if request.paused {
            return Err("request was paused by the operator".into());
        }
        if plan.tasks.is_empty() {
            return Err("completion intake must account for the full user request".into());
        }
        for cancellation in &plan.cancellations {
            if cancellation.quote.trim().is_empty() || !request.prompt.contains(&cancellation.quote) {
                return Err("task cancellation must cite the current user's exact instruction".into());
            }
            if !state.tasks.iter().any(|task| task.id == cancellation.task_id && !task.status.terminal()) {
                return Err(format!("cancellation references no open task: {}", cancellation.task_id));
            }
        }
        for item in &plan.tasks {
            if item.text.trim().is_empty() || item.criteria.is_empty()
                || item.criteria.iter().any(|criterion| criterion.trim().is_empty()) {
                return Err("every task needs a title and concrete acceptance requirements before execution".into());
            }
        }
        for cancellation in plan.cancellations {
            if let Some(task) = state.tasks.iter_mut().find(|task| task.id == cancellation.task_id) {
                task.status = TaskStatus::Cancelled;
                task.reason = Some(format!("User request {request_id}: {}", cancellation.quote));
            }
        }
        for item in plan.tasks {
            state.tasks.push(WorkTask {
                id: uuid::Uuid::new_v4().to_string(),
                request_id: request_id.to_string(),
                phase: "Tasks".into(),
                text: item.text,
                criteria: item.criteria,
                kind: item.kind,
                origin: TaskOrigin::User,
                status: TaskStatus::Pending,
                reason: None,
                verification: None,
            });
        }
        state.requests.iter_mut().find(|request| request.id == request_id)
            .expect("request checked above").planned = true;
        state.blocker = None;
        Ok(())
    })?;
    Ok(state)
}

pub(crate) fn observed_blocker(session: &Path, operation: &str, message: &str) -> Result<CompletionState, String> {
    let (_, state) = store::update(session, None, |state| {
        state.blocker = Some(CompletionBlocker {
            operation: operation.to_string(),
            message: message.to_string(),
            observed_at: crate::agent::now_stamp(),
        });
        Ok(())
    })?;
    Ok(state)
}

pub(crate) fn clear_runtime_blocker(session: &Path) -> Result<CompletionState, String> {
    store::update(session, None, |state| {
        state.blocker = None;
        Ok(())
    }).map(|(_, state)| state)
}

/// This operation is exposed to operator CLI/RPC clients, never as an agent tool.
/// There is deliberately no operator or model operation named `done` or `pass`.
pub(crate) fn operator_control(
    session: &Path,
    task_id: &str,
    action: &str,
    reason: &str,
    revision: u64,
) -> Result<CompletionState, String> {
    if reason.trim().is_empty() {
        return Err("task control requires a nonempty operator reason".into());
    }
    let status = match action {
        "cancel" => TaskStatus::Cancelled,
        "pause" => TaskStatus::Paused,
        "resume" => TaskStatus::Pending,
        _ => return Err(format!("unsupported operator task action: {action}")),
    };
    let (_, state) = store::update(session, Some(revision), |state| {
        if let Some(request_index) = state.requests.iter().position(|request| request.id == task_id) {
            let request = &mut state.requests[request_index];
            if request.coverage_verified {
                return Err(format!("request is already terminal: {task_id}"));
            }
            request.paused = action == "pause";
            if action == "cancel" && !request.planned {
                state.tasks.push(WorkTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    request_id: request.id.clone(),
                    phase: "Requests".into(),
                    text: request.prompt.lines().next().unwrap_or(&request.prompt).to_string(),
                    criteria: vec![request.prompt.clone()],
                    kind: TaskKind::Work,
                    origin: TaskOrigin::User,
                    status: TaskStatus::Cancelled,
                    reason: Some(format!("Operator cancel: {reason}")),
                    verification: None,
                });
                request.planned = true;
            }
            for task in state.tasks.iter_mut().filter(|task| task.request_id == task_id && !task.status.terminal()) {
                task.status = status;
                task.reason = Some(format!("Operator {action}: {reason}"));
                task.verification = None;
            }
        } else {
            let task = state.tasks.iter_mut().find(|task| task.id == task_id)
                .ok_or_else(|| format!("unknown task or request: {task_id}"))?;
            if task.status.terminal() {
                return Err(format!("task is already terminal: {task_id}"));
            }
            task.status = status;
            task.reason = Some(format!("Operator {action}: {reason}"));
            task.verification = None;
        }
        state.blocker = None;
        for request in &mut state.requests {
            let mut owned = state.tasks.iter().filter(|task| task.request_id == request.id).peekable();
            if owned.peek().is_some() && owned.all(|task| task.status == TaskStatus::Cancelled) {
                request.coverage_verified = true;
            }
        }
        Ok(())
    })?;
    Ok(state)
}

pub fn snapshot(session: &Path) -> Result<Value, String> {
    let state = store::read(session)?;
    Ok(snapshot_value(&state))
}

pub(crate) fn snapshot_value(state: &CompletionState) -> Value {
    json!({
        "schemaVersion": state.schema_version,
        "revision": state.revision,
        "status": state.status(),
        "complete": state.complete(),
        "requests": state.requests,
        "tasks": state.tasks,
        "blocker": state.blocker,
        "completed": state.tasks.iter().filter(|task| task.status == TaskStatus::Done).count(),
        "cancelled": state.tasks.iter().filter(|task| task.status == TaskStatus::Cancelled).count(),
        "unplanned": state.requests.iter().filter(|request| !request.planned).count(),
        "total": state.tasks.len() + state.requests.iter().filter(|request| !request.planned).count(),
        "open": state.tasks.iter().filter(|task| !task.status.terminal()).count()
            + state.requests.iter().filter(|request| !request.planned).count(),
    })
}

pub(crate) fn model_context(state: &CompletionState) -> String {
    let requests: Vec<_> = state.requests.iter()
        .filter(|request| !request.coverage_verified)
        .map(|request| json!({"id": request.id, "prompt": request.prompt, "cwd": request.cwd, "paused": request.paused, "planned": request.planned}))
        .collect();
    let tasks: Vec<_> = state.tasks.iter().filter(|task| !task.status.terminal()).collect();
    format!(
        "Jeden owns completion of every retained user request. New questions do not cancel earlier work. \
         Acceptance requirements below were recorded before execution. `todo done` only requests independent verification; \
         it does not complete a task. Do not repeat effects already proven by the session. \
         Use a message action for progress or an answer that does not finish the retained work. \
         A final action is a completion proposal and will be checked against ALL open requests.\n{}",
        json!({"requests": requests, "tasks": tasks, "blocker": state.blocker})
    )
}
