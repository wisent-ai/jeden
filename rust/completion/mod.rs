//! The harness owns obligations and acceptance; model todo updates are proposals.
//! `completion.json` is the atomic session-owned source of task state. Session
//! events retain decisions and reviewer tool receipts rather than another ledger
//! that can independently decide whether the work is complete.

mod asks;
pub(crate) mod cli;
mod constants;
mod model;
mod operations;
mod store;
pub(crate) mod timing;
mod todo;
mod verification;

pub(crate) use asks::ask_user::{recall as recall_question, record as record_question};
pub(crate) use asks::cli::command as asks_command;
pub(crate) use asks::cli::{
    answer_value as answer_ask, list_value as list_asks, show_value as show_ask,
};
pub(crate) use asks::{deliver_answers, open_asks, session_key as ask_session_key};

/// The register as the models read it: every ask and its answer.
pub(crate) fn asks_context() -> Result<Vec<serde_json::Value>, String> {
    asks::read().map(|register| register.context())
}
pub(crate) use cli::command;
pub use constants::SCHEMA_VERSION;
pub use model::{
    CompletionBlocker, CompletionEstimate, CompletionState, CriterionReview, EvidenceReference,
    TaskKind, TaskOrigin, TaskStatus, TaskVerification, WorkRequest, WorkTask,
};
pub use operations::snapshot;

pub(crate) use model::{unreadable, CompletionReview, IntakePlan};
pub(crate) use operations::{
    capture_request, clear_runtime_blocker, model_context, observed_blocker, operator_control,
    plan_request, snapshot_value,
};
pub(crate) use store::{inherit, migrate_workspace, read as read_state, session_from_artifacts};
pub(crate) use todo::execute as execute_todo;
pub(crate) use verification::{apply_review, inspect_evidence, review_evidence};
