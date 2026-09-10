//! The harness owns obligations and acceptance; model todo updates are proposals.
//! `completion.json` is the atomic session-owned source of task state. Session
//! events retain decisions and reviewer tool receipts rather than another ledger
//! that can independently decide whether the work is complete.

pub(crate) mod cli;
mod constants;
mod model;
mod operations;
mod store;
mod todo;
mod verification;

pub use constants::SCHEMA_VERSION;
pub use model::{
    CompletionBlocker, CompletionState, EvidenceReference, TaskKind, TaskOrigin,
    TaskStatus, TaskVerification, WorkRequest, WorkTask, CriterionReview,
};
pub use operations::snapshot;
pub(crate) use cli::command;

pub(crate) use model::{CompletionReview, IntakePlan};
pub(crate) use operations::{
    capture_request, clear_runtime_blocker, model_context, observed_blocker, operator_control, plan_request,
    snapshot_value,
};
pub(crate) use store::{inherit, read as read_state, session_from_artifacts};
pub(crate) use todo::execute as execute_todo;
pub(crate) use verification::{apply_review, inspect_evidence, review_evidence};
