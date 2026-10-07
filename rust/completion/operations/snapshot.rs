use super::*;

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
        "timing": super::super::timing::values(state),
        "completed": state.tasks.iter().filter(|task| task.status == TaskStatus::Done).count(),
        "cancelled": state.tasks.iter().filter(|task| task.status == TaskStatus::Cancelled).count(),
        "unplanned": state.requests.iter().filter(|request| !request.planned).count(),
        "total": state.tasks.len() + state.requests.iter().filter(|request| !request.planned).count(),
        "open": state.tasks.iter().filter(|task| !task.status.terminal()).count()
            + state.requests.iter().filter(|request| !request.planned).count(),
    })
}

pub(crate) fn model_context(state: &CompletionState) -> Result<String, String> {
    let now = super::super::timing::now();
    let asks = super::super::asks::read()?.context();
    let requests: Vec<_> = state.requests.iter()
        .filter(|request| !request.coverage_verified)
        .map(|request| {
            let mut value = json!({"id": request.id, "prompt": request.prompt, "cwd": request.cwd, "paused": request.paused, "planned": request.planned});
            if let Some(timing) = super::super::timing::context(request, now) {
                value["timeToCompletion"] = timing;
            }
            value
        })
        .collect();
    let tasks: Vec<_> = state
        .tasks
        .iter()
        .filter(|task| !task.status.terminal())
        .collect();
    Ok(format!(
        "Jeden owns completion of every retained user request. New questions do not cancel earlier work. \
         Acceptance requirements below were recorded before execution. `todo done` only requests independent verification; \
         it does not complete a task. Do not repeat effects already proven by the session. \
         Use a message action for progress or an answer that does not finish the retained work. \
         A task with an operatorRequest waits on the operator: an unanswered ask is theirs to answer, not yours to work around; \
         an answered one carries their answer, so use it and do not ask again. \
         operatorAsks is the register of everything the operator has been asked in any session and what they answered: \
         never ask the operator, in a question, a message or a final answer, for something it already holds an answer to; use that answer. \
         An ask there without an answer is already in front of the operator; say so instead of asking again. \
         A planned request's timeToCompletion is the estimate the intake recorded before execution and the minutes spent since: \
         tell the user that estimate when you start the work and never revise it; Jeden measures the actual time and adds it to the final answer. \
         A final action is a completion proposal and will be checked against ALL open requests.\n{}",
        json!({"requests": requests, "tasks": tasks, "blocker": state.blocker, "operatorAsks": asks})
    ))
}
