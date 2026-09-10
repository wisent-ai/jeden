use serde::{Deserialize, Serialize};

use super::constants::{INITIAL_REVISION, SCHEMA_VERSION};

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    pub origin: TaskOrigin,
    pub status: TaskStatus,
    pub reason: Option<String>,
    pub verification: Option<TaskVerification>,
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
        if let Some(blocker) = &self.blocker {
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
        }
        for request in self.requests.iter().filter(|request| request.planned) {
            if !self.tasks.iter().any(|task| task.request_id == request.id) {
                return Err(format!("request {} has no acceptance tasks", request.id));
            }
        }
        Ok(())
    }
}

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
    pub kind: TaskKind,
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
    pub evidence: Vec<EvidenceReference>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CriterionReview {
    pub index: usize,
    pub satisfied: bool,
    pub explanation: String,
    pub evidence: Vec<EvidenceReference>,
}
