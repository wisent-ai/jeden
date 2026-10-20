mod answers;
mod defects;
mod requests;
mod snapshot;
pub(crate) use requests::capture_request;
pub use snapshot::snapshot;
pub(crate) use snapshot::{model_context, snapshot_value};

use super::{model::*, store};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) fn plan_request(
    session: &Path,
    revision: u64,
    request_id: &str,
    plan: IntakePlan,
) -> Result<CompletionState, String> {
    let recorded_at = crate::agent::now_stamp()
        .map_err(|error| format!("the plan's estimate cannot be dated: {error}"))?;
    let (_, state) = store::update(session, Some(revision), |state| {
        let request = state
            .requests
            .iter()
            .find(|request| request.id == request_id)
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
        let minutes = plan
            .estimate_minutes
            .filter(|minutes| *minutes > 0)
            .ok_or("completion intake must estimate the time to completion before execution: estimateMinutes is a whole number of minutes above zero")?;
        for cancellation in &plan.cancellations {
            if cancellation.quote.trim().is_empty() || !request.prompt.contains(&cancellation.quote)
            {
                return Err(
                    "task cancellation must cite the current user's exact instruction".into(),
                );
            }
            if !state
                .tasks
                .iter()
                .any(|task| task.id == cancellation.task_id && !task.status.terminal())
            {
                return Err(format!(
                    "cancellation references no open task: {}",
                    cancellation.task_id
                ));
            }
        }
        for item in &plan.tasks {
            if (item.kind == TaskKind::Defect) != item.defect_of.is_some()
                || item.kind == TaskKind::Defect
                    && !item.defect_quote.as_ref().is_some_and(|quote| {
                        !quote.trim().is_empty() && request.prompt.contains(quote)
                    })
            {
                return Err("a defect requires its original task id and an exact quote from the current user's defect report".into());
            }
            if item.text.trim().is_empty()
                || item.criteria.is_empty()
                || item
                    .criteria
                    .iter()
                    .any(|criterion| criterion.trim().is_empty())
            {
                return Err("every task needs a title and concrete acceptance requirements before execution".into());
            }
        }
        for cancellation in plan.cancellations {
            if let Some(task) = state
                .tasks
                .iter_mut()
                .find(|task| task.id == cancellation.task_id)
            {
                task.status = TaskStatus::Cancelled;
                task.reason = Some(format!("User request {request_id}: {}", cancellation.quote));
            }
        }
        for item in plan.tasks {
            let (defect_of, status) = if let Some(target) = item.defect_of {
                let (original, _, status) = defects::reopen_target(state, &target, &item.text)?;
                (Some(original), status)
            } else {
                (None, TaskStatus::Pending)
            };
            state.tasks.push(WorkTask {
                id: uuid::Uuid::new_v4().to_string(),
                request_id: request_id.to_string(),
                phase: "Tasks".into(),
                text: item.text,
                criteria: item.criteria,
                kind: item.kind,
                defect_of,
                origin: TaskOrigin::User,
                status,
                reason: None,
                verification: None,
                operator_request: None,
            });
        }
        let request = state
            .requests
            .iter_mut()
            .find(|request| request.id == request_id)
            .expect("request checked above");
        request.planned = true;
        request.estimate = Some(CompletionEstimate {
            minutes,
            recorded_at: recorded_at.clone(),
        });
        state.blocker = None;
        Ok(())
    })?;
    Ok(state)
}

pub(crate) fn observed_blocker(
    session: &Path,
    operation: &str,
    message: &str,
) -> Result<CompletionState, String> {
    let observed_at = crate::agent::now_stamp()
        .map_err(|error| format!("the {operation} blocker cannot be dated: {error}"))?;
    let (_, state) = store::update(session, None, |state| {
        if operation == "turn_cancelled" && (state.complete() || state.status() == "paused") {
            return Ok(());
        }
        state.blocker = Some(CompletionBlocker {
            operation: operation.to_string(),
            message: message.to_string(),
            observed_at: observed_at.clone(),
        });
        Ok(())
    })?;
    Ok(state)
}

pub(crate) fn clear_runtime_blocker(session: &Path) -> Result<CompletionState, String> {
    store::update(session, None, |state| {
        state.blocker = None;
        Ok(())
    })
    .map(|(_, state)| state)
}

/// What the operator does to a task or request, spelled by its serde name on
/// the CLI (`jeden todo <action>`) and in RPC `session/completion/control`.
/// There is deliberately no operator or model action named `done` or `pass`.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlAction {
    Pause,
    Resume,
    Cancel,
    Defect,
    Answer,
}

impl ControlAction {
    /// The action `word` names, refused naming the word and every action.
    pub fn named(word: &str) -> Result<Self, String> {
        crate::vocabulary::named(word)
            .map_err(|cause| format!("unsupported operator task action: {word} ({cause})"))
    }

    pub fn name(self) -> String {
        crate::vocabulary::spelling(self)
    }
}

/// This operation is exposed to operator CLI/RPC clients, never as an agent tool.
pub(crate) fn operator_control(
    session: &Path,
    task_id: &str,
    action: ControlAction,
    reason: &str,
    revision: u64,
) -> Result<CompletionState, String> {
    if reason.trim().is_empty() {
        return Err("task control requires a nonempty operator reason".into());
    }
    let status = match action {
        ControlAction::Defect => return defects::report(session, task_id, reason, revision),
        ControlAction::Answer => return answers::record(session, task_id, reason, revision),
        ControlAction::Cancel => TaskStatus::Cancelled,
        ControlAction::Pause => TaskStatus::Paused,
        ControlAction::Resume => TaskStatus::Pending,
    };
    let action_name = action.name();
    let (_, state) = store::update(session, Some(revision), |state| {
        if let Some(request_index) = state
            .requests
            .iter()
            .position(|request| request.id == task_id)
        {
            let request = &mut state.requests[request_index];
            if request.coverage_verified {
                return Err(format!("request is already terminal: {task_id}"));
            }
            request.paused = action == ControlAction::Pause;
            if action == ControlAction::Cancel {
                request.coverage_verified = true;
            }
            if action == ControlAction::Cancel && !request.planned {
                state.tasks.push(WorkTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    request_id: request.id.clone(),
                    phase: "Requests".into(),
                    // The prompt's first line; a prompt of one line is its own.
                    text: match request.prompt.split_once('\n') {
                        Some((first, _)) => first.trim_end_matches('\r'),
                        None => request.prompt.as_str(),
                    }
                    .to_string(),
                    criteria: vec![request.prompt.clone()],
                    kind: TaskKind::Work,
                    defect_of: None,
                    origin: TaskOrigin::User,
                    status: TaskStatus::Cancelled,
                    reason: Some(format!("Operator cancel: {reason}")),
                    verification: None,
                    operator_request: None,
                });
                request.planned = true;
            }
            for task in state
                .tasks
                .iter_mut()
                .filter(|task| task.request_id == task_id && !task.status.terminal())
            {
                task.status = status;
                task.reason = Some(format!("Operator {action_name}: {reason}"));
                task.verification = None;
            }
        } else {
            let task = state
                .tasks
                .iter_mut()
                .find(|task| task.id == task_id)
                .ok_or_else(|| format!("unknown task or request: {task_id}"))?;
            if task.status.terminal() {
                return Err(format!("task is already terminal: {task_id}"));
            }
            if action == ControlAction::Resume
                && state
                    .requests
                    .iter()
                    .any(|request| request.id == task.request_id && request.paused)
            {
                return Err("the owning request is paused; resume that request first".into());
            }
            task.status = status;
            task.reason = Some(format!("Operator {action_name}: {reason}"));
            task.verification = None;
        }
        state.blocker = None;
        for request in &mut state.requests {
            let mut owned = state
                .tasks
                .iter()
                .filter(|task| task.request_id == request.id)
                .peekable();
            if owned.peek().is_some() && owned.all(|task| task.status == TaskStatus::Cancelled) {
                request.coverage_verified = true;
            }
        }
        Ok(())
    })?;
    Ok(state)
}
