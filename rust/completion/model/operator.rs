//! What a blocked task asks of the operator, and what they answered.

use serde::{Deserialize, Serialize};

use super::{CompletionState, TaskStatus, WorkTask};

/// The one thing a blocked task waits on that only the operator holds: a
/// value, a credential, a decision. Recorded on the task in the operator's
/// own words to read, answered by the operator through `jeden todo answer`,
/// and shown wherever the task is. A block that names no such thing is not
/// waiting on the operator and stays a plain blocker with its failed
/// operation.
///
/// On 2026-09-18 an agent sat behind the same blocker for a whole afternoon,
/// answered the Stop guard sixteen times with the same paragraph, and the
/// operator had to ask "what exactly are you waiting for". The answer was
/// three concrete things that existed in no file. This record is where they
/// go, so the next session reads them instead of rediscovering them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorRequest {
    pub asked_at: String,
    /// The exact value or decision wanted, and where it goes.
    pub ask: String,
    #[serde(default)]
    pub answer: Option<OperatorAnswer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorAnswer {
    pub answered_at: String,
    pub text: String,
}

impl WorkTask {
    /// Blocked on something the operator has been asked for and has not yet
    /// answered.
    pub fn waits_for_operator(&self) -> bool {
        self.status == TaskStatus::Blocked
            && self
                .operator_request
                .as_ref()
                .is_some_and(|request| request.answer.is_none())
    }
}

impl CompletionState {
    /// Every unanswered ask, each with the command that answers it, so a
    /// stop on `waiting_for_operator` tells the operator what to do and not
    /// only that something is waited on.
    pub fn open_asks(&self) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|task| task.waits_for_operator())
            .filter_map(|task| task.operator_request.as_ref().map(|request| (task, request)))
            .map(|(task, request)| {
                format!(
                    "Waiting on you: {} Answer with: jeden todo answer {} --text <answer> --revision {}",
                    request.ask, task.id, self.revision
                )
            })
            .collect()
    }
}
