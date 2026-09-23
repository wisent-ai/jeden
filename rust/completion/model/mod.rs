use serde::{Deserialize, Serialize};

use super::constants::{INITIAL_REVISION, SCHEMA_VERSION};

mod operator;
mod review;
mod criterion;
pub use criterion::CriterionReview;

pub use operator::{OperatorAnswer, OperatorRequest};
pub(crate) use review::{unreadable, CompletionReview, IntakePlan, ReviewStatus, TaskReview};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    VerificationRequested,
    Done,
    Blocked,
    Paused,
    Cancelled,
}

impl TaskStatus {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOrigin {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Work,
    Answer,
    Defect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// A model answer names evidence in this shape, so an extra field it echoes is
// ignored here for the same reason as in `review.rs`.
#[serde(rename_all = "camelCase")]
pub struct EvidenceReference {
    pub session_path: String,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskVerification {
    pub reviewed_at: String,
    pub reviewer_session: String,
    pub explanation: String,
    pub evidence: Vec<EvidenceReference>,
    pub criteria: Vec<CriterionReview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkTask {
    pub id: String,
    pub request_id: String,
    pub phase: String,
    pub text: String,
    pub criteria: Vec<String>,
    pub kind: TaskKind,
    #[serde(default)]
    pub defect_of: Option<String>,
    pub origin: TaskOrigin,
    pub status: TaskStatus,
    pub reason: Option<String>,
    pub verification: Option<TaskVerification>,
    /// What the operator has to supply for a blocked task, when the block is
    /// theirs to lift; absent for every other task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_request: Option<OperatorRequest>,
}

/// How long the independent intake expected a request to take, recorded once
/// when it planned the request, before any execution. Nothing revises it, so
/// the measured time is always compared with the first promise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionEstimate {
    pub minutes: u64,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkRequest {
    pub id: String,
    pub prompt: String,
    pub cwd: String,
    #[serde(default)]
    pub paused: bool,
    pub captured_at: String,
    pub planned: bool,
    pub coverage_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<CompletionEstimate>,
    /// When an independent review accepted the request as complete; removed
    /// again when a defect reopens it, because reopened work was not done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionBlocker {
    pub operation: String,
    pub message: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionState {
    pub schema_version: u32,
    pub revision: u64,
    pub requests: Vec<WorkRequest>,
    pub tasks: Vec<WorkTask>,
    pub blocker: Option<CompletionBlocker>,
}

impl Default for CompletionState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revision: INITIAL_REVISION,
            requests: Vec::new(),
            tasks: Vec::new(),
            blocker: None,
        }
    }
}

impl CompletionState {
    pub fn complete(&self) -> bool {
        self.blocker.is_none()
            && self
                .requests
                .iter()
                .all(|request| request.planned && request.coverage_verified && !request.paused)
            && self.tasks.iter().all(|task| task.status.terminal())
    }

    pub fn actionable(&self) -> bool {
        self.requests
            .iter()
            .any(|request| !request.planned && !request.paused)
            || self.tasks.iter().any(|task| {
                self.requests
                    .iter()
                    .any(|request| request.id == task.request_id && !request.paused)
                    && matches!(
                        task.status,
                        TaskStatus::Pending
                            | TaskStatus::InProgress
                            | TaskStatus::VerificationRequested
                    )
            })
    }

    pub fn status(&self) -> &'static str {
        // Nothing moves until the operator answers, whatever else stopped
        // the last turn: a broken review or a step limit beside an open ask
        // is still a session waiting on them, and the word must say so.
        if self.tasks.iter().any(WorkTask::waits_for_operator) {
            "waiting_for_operator"
        } else if let Some(blocker) = &self.blocker {
            match blocker.operation.as_str() {
                "execution_limit" => "paused",
                "turn_cancelled" => "interrupted",
                _ => "blocked",
            }
        } else if self.complete() {
            if !self.tasks.is_empty()
                && self
                    .tasks
                    .iter()
                    .all(|task| task.status == TaskStatus::Cancelled)
            {
                "cancelled"
            } else {
                "complete"
            }
        } else if self
            .requests
            .iter()
            .any(|request| !request.planned && !request.paused)
        {
            "planning"
        } else if self.actionable() {
            "working"
        } else if self
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::Blocked)
        {
            "blocked"
        } else if self
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::Paused)
            || self.requests.iter().any(|request| request.paused)
        {
            "paused"
        } else {
            "verification_required"
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported completion state version: {}",
                self.schema_version
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for request in &self.requests {
            if request.id.trim().is_empty()
                || request.prompt.trim().is_empty()
                || request.cwd.trim().is_empty()
                || !ids.insert(request.id.as_str())
            {
                return Err("completion state contains an empty or duplicate request".into());
            }
            if request
                .estimate
                .as_ref()
                .is_some_and(|estimate| estimate.minutes == 0)
            {
                return Err(format!(
                    "request {} records a time-to-completion estimate of zero minutes",
                    request.id
                ));
            }
            if request.completed_at.is_some() && !request.coverage_verified {
                return Err(format!(
                    "request {} records a completion time but is not verified complete",
                    request.id
                ));
            }
        }
        ids.clear();
        for task in &self.tasks {
            if task.id.trim().is_empty()
                || task.text.trim().is_empty()
                || task.criteria.is_empty()
                || task.criteria.iter().any(|item| item.trim().is_empty())
                || !ids.insert(task.id.as_str())
            {
                return Err("completion state contains an invalid or duplicate task".into());
            }
            if !self
                .requests
                .iter()
                .any(|request| request.id == task.request_id)
            {
                return Err(format!("task {} references an unknown request", task.id));
            }
            if task.status == TaskStatus::Done && task.verification.is_none() {
                return Err(format!("task {} has no independent verification", task.id));
            }
            if let Some(target) = &task.defect_of {
                if task.kind != TaskKind::Defect
                    || target == &task.id
                    || !self.tasks.iter().any(|item| &item.id == target)
                        && !self.requests.iter().any(|item| &item.id == target)
                {
                    return Err(format!(
                        "task {} has an invalid defect target: {target}",
                        task.id
                    ));
                }
            } else if task.kind == TaskKind::Defect {
                return Err(format!(
                    "defect {} has no original task or request",
                    task.id
                ));
            }
        }
        for request in self.requests.iter().filter(|request| request.planned) {
            if !self.tasks.iter().any(|task| task.request_id == request.id) {
                return Err(format!("request {} has no acceptance tasks", request.id));
            }
        }
        Ok(())
    }
}
