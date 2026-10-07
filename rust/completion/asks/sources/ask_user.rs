//! Questions the execution agent puts to the operator with `ask_user`: each
//! one and its answer go into the register, and the same question in the same
//! workspace is answered from it instead of being asked again.

use super::super::{AskOccurrence, AskSource, OperatorAsk};
use crate::completion::model::OperatorAnswer;
use std::path::Path;

/// The answer the operator already gave to `question` in `workspace`, with
/// the place it was asked again recorded; `None` when it was never answered.
pub(crate) fn recall(
    workspace: &Path,
    session: &Path,
    question: &str,
) -> Result<Option<OperatorAsk>, String> {
    let workspace = workspace.display().to_string();
    let found = super::super::read()?
        .asks
        .iter()
        .any(|ask| matches(ask, &workspace, question));
    if !found {
        return Ok(None);
    }
    let session = super::super::session_key(session);
    super::super::update(|register| {
        let Some(ask) = register
            .asks
            .iter_mut()
            .rev()
            .find(|ask| matches(ask, &workspace, question))
        else {
            return Ok(None);
        };
        ask.occurrences.push(AskOccurrence {
            asked_at: crate::agent::now_stamp(),
            source: AskSource::AskUser,
            session_path: session,
            task_id: None,
            wording: question.to_string(),
            answered_from_register: true,
        });
        Ok(Some(ask.clone()))
    })
    .map(|(ask, _)| ask)
}

/// Records a question the operator has just answered; returns its ask id.
pub(crate) fn record(
    workspace: &Path,
    session: &Path,
    question: &str,
    answer: &str,
) -> Result<String, String> {
    let stamp = crate::agent::now_stamp();
    let id = uuid::Uuid::new_v4().to_string();
    super::super::update(|register| {
        register.asks.push(OperatorAsk {
            id: id.clone(),
            ask: question.to_string(),
            workspace: workspace.display().to_string(),
            asked_at: stamp.clone(),
            follows: None,
            occurrences: vec![AskOccurrence {
                asked_at: stamp.clone(),
                source: AskSource::AskUser,
                session_path: super::super::session_key(session),
                task_id: None,
                wording: question.to_string(),
                answered_from_register: false,
            }],
            answer: Some(OperatorAnswer {
                answered_at: stamp.clone(),
                text: answer.to_string(),
            }),
            oko: None,
        });
        Ok(())
    })?;
    Ok(id)
}

fn matches(ask: &OperatorAsk, workspace: &str, question: &str) -> bool {
    ask.answer.is_some()
        && ask.workspace == workspace
        && super::super::same_words(&ask.ask, question)
}
