use super::*;
use crate::completion::asks;

/// The operator answers what a blocked task asked, from the task: the answer
/// goes to the register ask the task waits on, and from there to this task
/// and to every other task, in any session, waiting on the same ask. Reached
/// through `operator_control` with action `answer`, where the operator's text
/// is the answer; never an agent tool: an agent that could answer its own ask
/// would have nothing to wait for.
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
    if text.trim().is_empty() {
        return Err("an answer requires nonempty text".into());
    }
    let state = store::read(session)?;
    if state.revision != revision {
        return Err(format!(
            "completion state changed: expected revision {revision}, found {}",
            state.revision
        ));
    }
    let task = state
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    if task.status.terminal() {
        return Err(format!("task is already terminal: {task_id}"));
    }
    let request = task.operator_request.as_ref().ok_or_else(|| {
        format!("task {task_id} asks nothing of the operator; resume it with a reason instead")
    })?;
    if request.answer.is_some() {
        return Err(format!(
            "task {task_id} already holds an answer to its request"
        ));
    }
    let key = asks::session_key(session);
    let ask_id = match asks::read()?.for_task(&key, task_id) {
        Some(ask) => ask.id.clone(),
        None => adopt(&state, task, request, &key)?,
    };
    let report = asks::answer(&ask_id, text, Some((session, revision)))?;
    if let Some(failed) = report.deliveries.iter().find(|delivery| {
        delivery.session_path == key
            && delivery.task_id == task_id
            && delivery.outcome != "delivered"
    }) {
        return Err(format!(
            "the answer to ask {ask_id} is recorded, but task {task_id} did not take it: {}",
            failed.detail
        ));
    }
    store::read(session)
}

/// A task asked before the register knew it, by an older Jeden: its ask
/// joins the register ask asked in the same words, or enters it as it was
/// asked, so the answer has a place to go.
fn adopt(
    state: &CompletionState,
    task: &WorkTask,
    request: &OperatorRequest,
    session: &str,
) -> Result<String, String> {
    let workspace = state
        .requests
        .iter()
        .find(|item| item.id == task.request_id)
        .map(|item| item.cwd.clone())
        .unwrap_or_default();
    let occurrence = asks::AskOccurrence {
        asked_at: request.asked_at.clone(),
        source: asks::AskSource::Review,
        session_path: session.to_string(),
        task_id: Some(task.id.clone()),
        wording: request.ask.clone(),
        answered_from_register: false,
    };
    asks::update(|register| {
        if let Some(ask) = register
            .asks
            .iter_mut()
            .rev()
            .find(|ask| asks::same_words(&ask.ask, &request.ask))
        {
            ask.occurrences.push(occurrence);
            return Ok(ask.id.clone());
        }
        let id = uuid::Uuid::new_v4().to_string();
        register.asks.push(asks::OperatorAsk {
            id: id.clone(),
            ask: request.ask.clone(),
            workspace,
            asked_at: request.asked_at.clone(),
            follows: None,
            occurrences: vec![occurrence],
            answer: None,
            oko: None,
        });
        Ok(id)
    })
    .map(|(id, _)| id)
    .and_then(|id| asks::put_waiting_on_oko(std::slice::from_ref(&id)).map(|()| id))
}
