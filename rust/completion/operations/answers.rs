use super::*;

/// The operator answers a blocked task's request: the answer is recorded
/// beside the ask, the task returns to pending, and the next `continue`
/// reads both. Reached through `operator_control` with action `answer`,
/// where the operator's text is the answer; never an agent tool: an agent
/// that could answer its own ask would have nothing to wait for.
///
/// Refused for a task that asked nothing: an answer to no question is a
/// note, and `resume` with a reason already exists for that. Refused for a
/// request that is already answered: the second answer would overwrite the
/// first with no record of either having been read.
pub(super) fn record(
    session: &Path,
    task_id: &str,
    text: &str,
    revision: u64,
) -> Result<CompletionState, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("an answer requires nonempty text".into());
    }
    let (_, state) = store::update(session, Some(revision), |state| {
        let task = state
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
            .ok_or_else(|| format!("unknown task: {task_id}"))?;
        if task.status.terminal() {
            return Err(format!("task is already terminal: {task_id}"));
        }
        let request = task
            .operator_request
            .as_mut()
            .ok_or_else(|| format!("task {task_id} asks nothing of the operator; resume it with a reason instead"))?;
        if request.answer.is_some() {
            return Err(format!("task {task_id} already holds an answer to its request"));
        }
        request.answer = Some(OperatorAnswer {
            answered_at: crate::agent::now_stamp(),
            text: text.to_string(),
        });
        task.status = TaskStatus::Pending;
        task.reason = Some(format!("Operator answered: {text}"));
        task.verification = None;
        state.blocker = None;
        Ok(())
    })?;
    Ok(state)
}
