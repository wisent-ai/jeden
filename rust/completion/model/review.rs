//! What a model answer may say about retained work: the intake plan a fresh
//! inspection returns, and the acceptance review a fresh verifier returns.
//!
//! These shapes are read, never written: the persisted state in the parent
//! module is what the product keeps. A missing field here is one answer being
//! poorer than asked for, so each default below is the reading that cannot
//! make completion cheaper than the request.

use serde::Deserialize;

use super::{CriterionReview, EvidenceReference, TaskKind};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct IntakePlan {
    pub tasks: Vec<PlannedTask>,
    #[serde(default)]
    pub cancellations: Vec<UserCancellation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UserCancellation {
    pub task_id: String,
    pub quote: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompletionReview {
    pub tasks: Vec<TaskReview>,
    pub requests: Vec<RequestReview>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskReview {
    pub task_id: String,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RequestReview {
    pub request_id: String,
    pub covered: bool,
    pub explanation: String,
}
