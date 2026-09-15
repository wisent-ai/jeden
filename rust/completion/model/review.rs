//! What a model answer may say about retained work: the intake plan a fresh
//! inspection returns, and the acceptance review a fresh verifier returns.
//!
//! These shapes are read, never written: the persisted state in the parent
//! module is what the product keeps. A missing field here is one answer being
//! poorer than asked for, so each default below is the reading that cannot
//! make completion cheaper than the request.
//!
//! An extra field is a different matter and is ignored. A verifier that echoes
//! `requestId` beside `taskId` has not changed its verdict, yet refusing the
//! whole answer for it cost a real run its budget: the corrections it spent on
//! `unknown field \`requestId\`` ended the turn with `Work remains open
//! (acceptance_review)` while the work itself was done. Every required field is
//! still required, so a misspelled `taskId` is refused as the missing field it
//! leaves behind.

use serde::Deserialize;

use super::{CriterionReview, EvidenceReference, TaskKind};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntakePlan {
    pub tasks: Vec<PlannedTask>,
    #[serde(default)]
    pub cancellations: Vec<UserCancellation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedTask {
    pub text: String,
    pub criteria: Vec<String>,
    /// An intake answer that omits the classification is read as work, the
    /// stricter of the two kinds: a work task is accepted only against an
    /// independent read-only observation, so a missing `kind` can never make
    /// completion cheaper than the request asked for. Refusing the whole
    /// intake instead left the retained request behind a durable blocker
    /// because one field was absent from an otherwise usable plan.
    #[serde(default = "work_task")]
    pub kind: TaskKind,
}

fn work_task() -> TaskKind {
    TaskKind::Work
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserCancellation {
    #[serde(alias = "id", alias = "task", alias = "task_id")]
    pub task_id: String,
    pub quote: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionReview {
    pub tasks: Vec<TaskReview>,
    pub requests: Vec<RequestReview>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskReview {
    /// Absent when a verifier answered about the only open task without
    /// repeating its identifier; the controller binds that one case and
    /// refuses every ambiguous one by name.
    #[serde(default, alias = "id", alias = "task", alias = "task_id")]
    pub task_id: Option<String>,
    pub status: ReviewStatus,
    pub explanation: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceReference>,
    /// A review that lists no criterion has reviewed no criterion. Reading the
    /// absent field as an empty list keeps the task unverified, which is the
    /// strict reading: the controller accepts `done` only when every recorded
    /// criterion is satisfied by an observation. On 2026-09-10 the opposite
    /// reading cost a whole retained assignment, which ended as `Work remains
    /// open (acceptance_review): invalid acceptance review: missing field
    /// `criteria``, with the work done and the review unread.
    #[serde(default)]
    pub criteria: Vec<CriterionReview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewStatus {
    Done,
    Continue,
    Blocked,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestReview {
    #[serde(alias = "id", alias = "request", alias = "request_id")]
    pub request_id: String,
    pub covered: bool,
    pub explanation: String,
}

/// A refusal a person can act on: the parser's own sentence plus the piece of
/// the answer it stopped at.
///
/// Twice on 2026-09-15 a finished run ended on `invalid acceptance review:
/// missing field \`taskId\` at line 1 column 2408`, and finding out what the
/// verifier had actually written meant reading the retained inspection session
/// by hand. The column is only useful beside the text it points into.
pub(crate) fn unreadable(kind: &str, answer: &str, error: &serde_json::Error) -> String {
    let reach = super::super::constants::REFUSAL_EXCERPT_CHARS;
    let start = error.column().saturating_sub(reach);
    let near: String = answer.chars().skip(start).take(reach * 2).collect();
    format!("invalid {kind}: {error}; the answer reads: {near}")
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
