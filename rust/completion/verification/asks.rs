//! What a review may ask of the operator on a blocked task, and how that
//! ask is kept on the task.

use super::super::model::{OperatorRequest, ReviewStatus, TaskReview, WorkTask};

fn named(verdict: &TaskReview) -> Option<&str> {
    verdict
        .ask
        .as_deref()
        .map(str::trim)
        .filter(|ask| !ask.is_empty() && verdict.status == ReviewStatus::Blocked)
}

/// A blocked verdict is accepted with a recorded failed operation or with
/// an ask, and refused with neither.
///
/// A value only the operator holds fails no operation: nothing was tried,
/// because there was nothing to try it with. Such a block is proven by the
/// ask it records, which the operator sees and answers. The operator's
/// answer is their input to the work: once it is recorded, the task is
/// blocked again only by an operation that failed after the answer. A
/// review that asks again, however it words the ask, or cites a failure
/// older than the answer, has not read it. On 2026-09-18 three reviews in
/// a row re-asked for a token the operator had already given, each with
/// new wording, before this rule existed.
pub(super) fn check(
    task: &WorkTask,
    verdict: &TaskReview,
    observed_failure: bool,
    failed_since_answer: bool,
) -> Result<(), String> {
    if verdict.status != ReviewStatus::Blocked {
        return Ok(());
    }
    if !observed_failure && named(verdict).is_none() {
        return Err(format!(
            "task {} was called blocked without a recorded failed operation or an ask of the operator",
            task.id
        ));
    }
    match task
        .operator_request
        .as_ref()
        .and_then(|request| request.answer.as_ref())
    {
        Some(answer) if !failed_since_answer => Err(format!(
            "task {} is blocked after the operator answered its ask with: {}; no operation failed since, so the work continues with that answer",
            task.id, answer.text
        )),
        _ => Ok(()),
    }
}

/// A blocked verdict that names what the operator must supply records that
/// ask on the task; the same ask worded the same keeps its date. A verdict
/// that is not blocked drops an unanswered ask, which is moot, and keeps an
/// answered one beside the work it fed.
pub(super) fn record(task: &mut WorkTask, verdict: &TaskReview) {
    task.operator_request = match (named(verdict), task.operator_request.take()) {
        (Some(ask), Some(existing)) if existing.ask == ask => Some(existing),
        (Some(ask), _) => Some(OperatorRequest {
            asked_at: crate::agent::now_stamp(),
            ask: ask.to_string(),
            answer: None,
        }),
        (None, Some(existing))
            if existing.answer.is_some() || verdict.status == ReviewStatus::Blocked =>
        {
            Some(existing)
        }
        (None, _) => None,
    };
}
