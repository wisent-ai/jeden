use super::*;
use std::collections::BTreeSet;

/// A regression in an earlier repair belongs to the original affected task.
fn original_target(state: &CompletionState, target: &str) -> Result<String, String> {
    let mut current = target;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current) {
            return Err(format!("defect references form a cycle at {current}"));
        }
        match state.tasks.iter().find(|task| task.id == current) {
            Some(task) if task.status == TaskStatus::Cancelled => {
                return Err(format!(
                    "cancelled task cannot be reopened by a defect: {current}"
                ));
            }
            Some(task) if task.kind == TaskKind::Defect => {
                current = task
                    .defect_of
                    .as_deref()
                    .ok_or("defect has no original target")?;
            }
            _ => return Ok(current.to_owned()),
        }
    }
}

/// Reopen the original obligation, but never undo an operator pause or cancellation.
/// The caller adds the repair task in the same completion-store transaction.
pub(super) fn reopen_target(
    state: &mut CompletionState,
    target: &str,
    reason: &str,
) -> Result<(String, String, TaskStatus), String> {
    let target = original_target(state, target)?;
    let request_index = state
        .requests
        .iter()
        .position(|request| request.id == target);
    let task_index = state.tasks.iter().position(|task| task.id == target);
    let owner = if let Some(index) = request_index {
        if state.requests[index].planned
            && state
                .tasks
                .iter()
                .filter(|task| task.request_id == target)
                .all(|task| task.status == TaskStatus::Cancelled)
        {
            return Err(format!(
                "cancelled request cannot be reopened by a defect: {target}"
            ));
        }
        index
    } else if let Some(index) = task_index {
        state
            .requests
            .iter()
            .position(|request| request.id == state.tasks[index].request_id)
            .ok_or("defect target has no owning request")?
    } else {
        return Err(format!("unknown task or request: {target}"));
    };
    let paused = state.requests[owner].paused
        || task_index.is_some_and(|index| state.tasks[index].status == TaskStatus::Paused);
    let status = if paused {
        TaskStatus::Paused
    } else {
        TaskStatus::Pending
    };
    let request_id = state.requests[owner].id.clone();
    for (index, task) in state.tasks.iter_mut().enumerate() {
        if (task_index == Some(index) || (request_index.is_some() && task.request_id == request_id))
            && task.status != TaskStatus::Cancelled
        {
            if task.status != TaskStatus::Paused {
                task.status = status;
            }
            task.verification = None;
            task.reason = Some(format!("Reopened by defect: {reason}"));
        }
    }
    state.requests[owner].coverage_verified = false;
    state.blocker = None;
    Ok((target, request_id, status))
}

pub(super) fn report(
    session: &Path,
    target: &str,
    reason: &str,
    revision: u64,
) -> Result<CompletionState, String> {
    let reason = reason.trim();
    let (_, state) = store::update(session, Some(revision), |state| {
        let original = original_target(state, target)?;
        if state.tasks.iter().any(|task| {
            task.kind == TaskKind::Defect
                && !task.status.terminal()
                && task.defect_of.as_deref() == Some(original.as_str())
                && task.text == reason
        }) {
            return Err(format!("the same defect is already open for {original}"));
        }
        let (original, request_id, status) = reopen_target(state, &original, reason)?;
        state.tasks.push(WorkTask {
            id: uuid::Uuid::new_v4().to_string(),
            request_id,
            phase: "Defects".into(),
            text: reason.to_owned(),
            criteria: vec![format!("Repair the reported defect and verify the original requirement: {reason}")],
            kind: TaskKind::Defect,
            defect_of: Some(original),
            origin: TaskOrigin::User,
            status,
            reason: Some("A defect reopened the original obligation; earlier verification does not prove repair.".into()),
            verification: None,
            operator_request: None,
        });
        Ok(())
    })?;
    Ok(state)
}
