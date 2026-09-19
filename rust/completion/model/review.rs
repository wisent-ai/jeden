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
    pub defect_of: Option<String>,
    pub defect_quote: Option<String>,
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

/// The two arrays a review returns. A request verdict the verifier filed
/// inside `tasks` is still a request verdict: on 2026-09-18 one model put
/// `{"requestId":…,"covered":…}` as the last element of `tasks` in four
/// reviews out of ten, and each was refused as `missing field status` on
/// a task that was never a task. An entry naming a request and its
/// coverage is unambiguous wherever it sits, so it is read as one; every
/// field it needs is still required.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawReview")]
pub(crate) struct CompletionReview {
    pub tasks: Vec<TaskReview>,
    pub requests: Vec<RequestReview>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReview {
    tasks: Vec<serde_json::Value>,
    #[serde(default)]
    requests: Vec<RequestReview>,
}

impl TryFrom<RawReview> for CompletionReview {
    type Error = String;

    fn try_from(raw: RawReview) -> Result<Self, String> {
        let mut review = Self {
            tasks: Vec::with_capacity(raw.tasks.len()),
            requests: raw.requests,
        };
        for mut entry in raw.tasks {
            // The same confusion the other way round: a `requests` array
            // written inside the last task instead of beside `tasks`.
            if let Some(nested) = entry
                .get_mut("requests")
                .filter(|value| value.is_array())
                .map(serde_json::Value::take)
            {
                let nested: Vec<RequestReview> =
                    serde_json::from_value(nested).map_err(|error| error.to_string())?;
                review.requests.extend(nested);
            }
            let names_request = entry.get("requestId").is_some() && entry.get("covered").is_some();
            if names_request {
                review.requests.push(serde_json::from_value(entry).map_err(|error| error.to_string())?);
            } else {
                review.tasks.push(serde_json::from_value(entry).map_err(|error| error.to_string())?);
            }
        }
        Ok(review)
    }
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
    /// For a `blocked` verdict: the exact value or decision only the operator
    /// holds, in one sentence, and where it goes. Absent when the block is a
    /// dependency nobody has to be asked about. Recorded on the task as its
    /// request to the operator and answered through `jeden todo answer`.
    #[serde(default, alias = "operator_request", alias = "operatorRequest")]
    pub ask: Option<String>,
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
