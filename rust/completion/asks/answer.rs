//! The operator's answer: recorded once in the register, then handed to
//! every task, in every session, that waits on the ask.

use super::{session_key, AskRegister, AskSource, OperatorAsk};
use crate::completion::model::{CompletionState, OperatorAnswer, TaskStatus, WorkTask};
use crate::completion::store;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnswerReport {
    pub ask: OperatorAsk,
    pub deliveries: Vec<Delivery>,
}

/// What happened to the answer on one task that was asked.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Delivery {
    pub session_path: String,
    pub task_id: String,
    /// `delivered`, `not_waiting` or `failed`.
    pub outcome: &'static str,
    pub detail: String,
}

/// The `session_path` of the delivery line that answers the ask's Oko twin.
pub(crate) const OKO_DELIVERY: &str = "oko";

impl AnswerReport {
    /// Waiting tasks the answer did not reach; `jeden todo continue` in each
    /// session hands it over. The Oko line is not one of them: its failure is
    /// printed with Oko's reason and the answer still stands.
    pub fn failures(&self) -> Vec<&Delivery> {
        self.deliveries
            .iter()
            .filter(|delivery| delivery.outcome == "failed" && delivery.session_path != OKO_DELIVERY)
            .collect()
    }
}

/// Records `text` as the answer to ask `id` and hands it to every task that
/// waits on it. `expected` is the session and revision an operator answered
/// from, refused as stale before anything is written. The ask's Oko twin is
/// answered too, so Oko stops asking for it.
pub(crate) fn answer(
    id: &str,
    text: &str,
    expected: Option<(&Path, u64)>,
) -> Result<AnswerReport, String> {
    let mut report = record(id, text, expected)?;
    if let Some(oko_id) = report.ask.oko.as_ref().and_then(|oko| oko.oko_id.clone()) {
        let (outcome, detail) = match super::oko::mirror_answer(&oko_id, text.trim()) {
            Ok(detail) => ("delivered", detail),
            Err(error) => ("failed", error),
        };
        report.deliveries.push(Delivery {
            session_path: OKO_DELIVERY.into(),
            task_id: oko_id,
            outcome,
            detail,
        });
    }
    Ok(report)
}

fn record(
    id: &str,
    text: &str,
    expected: Option<(&Path, u64)>,
) -> Result<AnswerReport, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("an answer requires nonempty text".into());
    }
    if let Some((session, revision)) = expected {
        let state = store::read(session)?;
        if state.revision != revision {
            return Err(format!(
                "completion state changed: expected revision {revision}, found {}",
                state.revision
            ));
        }
    }
    let (ask, register) = super::update(|register| {
        let ask = register
            .asks
            .iter_mut()
            .find(|ask| ask.id == id)
            .ok_or_else(|| {
                format!("unknown ask: {id}; jeden asks list shows every recorded ask")
            })?;
        if let Some(answer) = &ask.answer {
            return Err(format!(
                "ask {id} already holds an answer, given at {}: {}",
                answer.answered_at, answer.text
            ));
        }
        ask.answer = Some(OperatorAnswer {
            answered_at: crate::agent::now_stamp(),
            text: text.to_string(),
        });
        Ok(ask.clone())
    })?;
    let mut deliveries = Vec::new();
    for (session, task_id) in waiting_places(&register, &ask) {
        let revision = expected
            .filter(|(path, _)| session_key(path) == session)
            .map(|(_, revision)| revision);
        let result = store::update(Path::new(&session), revision, |state| {
            Ok(deliver(state, &task_id, &ask))
        });
        deliveries.push(match result {
            Ok((true, _)) => Delivery {
                session_path: session,
                task_id,
                outcome: "delivered",
                detail: "the task returned to pending with the answer".into(),
            },
            Ok((false, state)) => Delivery {
                detail: state
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .map(|task| {
                        format!(
                            "the task no longer waits on this ask (status {})",
                            status(task)
                        )
                    })
                    .unwrap_or_else(|| "the session no longer holds this task".into()),
                session_path: session,
                task_id,
                outcome: "not_waiting",
            },
            Err(error) => Delivery {
                session_path: session,
                task_id,
                outcome: "failed",
                detail: error,
            },
        });
    }
    Ok(AnswerReport { ask, deliveries })
}

/// Hands every answer the register holds to the tasks of `session` still
/// waiting on it: an answer given in another session, or one whose delivery
/// failed, reaches the task before its work continues.
pub(crate) fn deliver_answers(session: &Path) -> Result<CompletionState, String> {
    take_answers_from_oko(session)?;
    let register = super::read()?;
    let key = session_key(session);
    let state = store::read(session)?;
    let answered: Vec<(String, OperatorAsk)> = state
        .tasks
        .iter()
        .filter(|task| task.waits_for_operator())
        .filter_map(|task| {
            register
                .for_task(&key, &task.id)
                .filter(|ask| ask.answer.is_some())
                .map(|ask| (task.id.clone(), ask.clone()))
        })
        .collect();
    if answered.is_empty() {
        return Ok(state);
    }
    store::update(session, None, |state| {
        for (task_id, ask) in &answered {
            deliver(state, task_id, ask);
        }
        Ok(())
    })
    .map(|(_, state)| state)
}

/// An answer the operator gave in Oko to an ask a task of `session` waits
/// on is recorded in the register and handed to every waiting task, as if
/// given with `jeden asks answer`. An Oko that cannot be asked leaves the
/// ask waiting, and its reason is returned for the caller to show.
fn take_answers_from_oko(session: &Path) -> Result<(), String> {
    let register = super::read()?;
    let key = session_key(session);
    let state = store::read(session)?;
    let waiting: Vec<(String, String)> = state
        .tasks
        .iter()
        .filter(|task| task.waits_for_operator())
        .filter_map(|task| register.for_task(&key, &task.id))
        .filter(|ask| ask.answer.is_none())
        .filter_map(|ask| Some((ask.id.clone(), ask.oko.as_ref()?.oko_id.clone()?)))
        .collect();
    for (id, oko_id) in waiting {
        if let Some(text) = super::oko::answered(&oko_id)? {
            record(&id, &text, None)?;
        }
    }
    Ok(())
}

fn waiting_places(register: &AskRegister, ask: &OperatorAsk) -> Vec<(String, String)> {
    let mut places: Vec<(String, String)> = Vec::new();
    for occurrence in ask
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.source == AskSource::Review)
    {
        let Some(task_id) = &occurrence.task_id else {
            continue;
        };
        let place = (occurrence.session_path.clone(), task_id.clone());
        if !places.contains(&place)
            && register
                .for_task(&place.0, task_id)
                .is_some_and(|current| current.id == ask.id)
        {
            places.push(place);
        }
    }
    places
}

fn deliver(state: &mut CompletionState, task_id: &str, ask: &OperatorAsk) -> bool {
    let Some(answer) = &ask.answer else {
        return false;
    };
    let Some(task) = state.tasks.iter_mut().find(|task| task.id == task_id) else {
        return false;
    };
    if !task.waits_for_operator() {
        return false;
    }
    if let Some(request) = task.operator_request.as_mut() {
        request.answer = Some(answer.clone());
    }
    task.status = TaskStatus::Pending;
    task.reason = Some(format!("Operator answered: {}", answer.text));
    task.verification = None;
    state.blocker = None;
    true
}

fn status(task: &WorkTask) -> String {
    serde_json::to_value(task.status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}
