mod evidence;

pub(crate) use evidence::{inspect as inspect_evidence, review_evidence};
use super::{model::*, store};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn canonical(path: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(crate::cli::sessions::session_dir_for(path))
        .map_err(|error| format!("cannot resolve evidence session {path}: {error}"))
}

struct EvidenceIndex {
    parent: PathBuf,
    reviewer: PathBuf,
    parent_receipts: BTreeMap<String, Value>,
    reviewer_receipts: BTreeMap<String, Value>,
}

impl EvidenceIndex {
    fn get(&self, reference: &EvidenceReference) -> Result<(&Value, bool), String> {
        let source = canonical(&reference.session_path)?;
        let (receipts, independent) = if source == self.reviewer {
            (&self.reviewer_receipts, true)
        } else if source == self.parent || self.parent_receipts.get(&reference.event_id)
            .and_then(|receipt| receipt.get("sessionPath")).and_then(Value::as_str)
            .is_some_and(|path| canonical(path).is_ok_and(|path| path == source)) {
            (&self.parent_receipts, false)
        } else {
            return Err("verification cited a session outside this execution and its independent review".into());
        };
        let receipt = receipts.get(&reference.event_id)
            .ok_or_else(|| format!("verification cited nonexistent tool evidence: {}", reference.event_id))?;
        Ok((receipt, independent))
    }

    fn observation(&self, reference: &EvidenceReference) -> Result<bool, String> {
        let (receipt, independent) = self.get(reference)?;
        let tool = receipt.get("tool").and_then(Value::as_str).unwrap_or_default();
        Ok(independent && receipt["failed"] == false
            && crate::agent::is_verification_read_tool(tool))
    }

    fn failure(&self, reference: &EvidenceReference) -> Result<bool, String> {
        self.get(reference).map(|(receipt, _)| receipt["failed"] == true)
    }
}

pub(crate) fn apply_review(
    session: &Path,
    reviewer: &Path,
    expected_revision: u64,
    review: CompletionReview,
) -> Result<CompletionState, String> {
    let index = EvidenceIndex {
        parent: std::fs::canonicalize(session).map_err(|error| error.to_string())?,
        reviewer: std::fs::canonicalize(reviewer).map_err(|error| error.to_string())?,
        parent_receipts: evidence::receipts(session)?,
        reviewer_receipts: evidence::receipts(reviewer)?,
    };
    if index.parent == index.reviewer {
        return Err("the execution conversation cannot verify its own completion".into());
    }
    let (_, state) = store::update(session, Some(expected_revision), |state| {
        let expected: BTreeSet<_> = state.tasks.iter()
            .filter(|task| !task.status.terminal() && task.status != TaskStatus::Paused)
            .map(|task| task.id.as_str()).collect();
        let reported: BTreeSet<_> = review.tasks.iter().map(|task| task.task_id.as_str()).collect();
        if expected != reported || reported.len() != review.tasks.len() {
            return Err("independent review must cover every open task exactly once".into());
        }
        let expected_requests: BTreeSet<_> = state.requests.iter()
            .filter(|request| request.planned && !request.coverage_verified && !request.paused)
            .map(|request| request.id.as_str()).collect();
        let reported_requests: BTreeSet<_> = review.requests.iter()
            .map(|request| request.request_id.as_str()).collect();
        if expected_requests != reported_requests || reported_requests.len() != review.requests.len() {
            return Err("independent review must compare every unresolved original user request".into());
        }
        for verdict in &review.tasks {
            let task = state.tasks.iter().find(|task| task.id == verdict.task_id)
                .expect("task set validated above");
            if verdict.explanation.trim().is_empty() || verdict.criteria.len() != task.criteria.len() {
                return Err(format!("review of {} must explain every acceptance criterion", task.id));
            }
            let mut checked = BTreeSet::new();
            for criterion in &verdict.criteria {
                if criterion.index >= task.criteria.len() || !checked.insert(criterion.index)
                    || criterion.explanation.trim().is_empty() {
                    return Err(format!("review of {} contains an invalid or duplicate criterion", task.id));
                }
                let mut observed = false;
                for reference in &criterion.evidence {
                    observed |= index.observation(reference)?;
                }
                if verdict.status == ReviewStatus::Done && (!criterion.satisfied
                    || (task.kind == TaskKind::Work && !observed)) {
                    return Err(format!("task {} criterion {} has no successful independent observation", task.id, criterion.index));
                }
            }
            let mut observed_failure = false;
            for reference in &verdict.evidence {
                observed_failure |= index.failure(reference)?;
            }
            if verdict.status == ReviewStatus::Blocked && !observed_failure {
                return Err(format!("task {} was called blocked without a recorded failed operation", task.id));
            }
        }
        if review.requests.iter().any(|request| request.explanation.trim().is_empty()) {
            return Err("request coverage decisions require an explanation".into());
        }
        for verdict in review.tasks {
            let task = state.tasks.iter_mut().find(|task| task.id == verdict.task_id)
                .expect("task set validated above");
            task.reason = Some(verdict.explanation.clone());
            task.status = match verdict.status {
                ReviewStatus::Done => TaskStatus::Done,
                ReviewStatus::Continue => TaskStatus::Pending,
                ReviewStatus::Blocked => TaskStatus::Blocked,
            };
            let mut references = verdict.evidence;
            for criterion in &verdict.criteria {
                for reference in &criterion.evidence {
                    if !references.contains(reference) {
                        references.push(reference.clone());
                    }
                }
            }
            task.verification = Some(TaskVerification {
                reviewed_at: crate::agent::now_stamp(),
                reviewer_session: reviewer.display().to_string(),
                explanation: verdict.explanation,
                evidence: references,
                criteria: verdict.criteria,
            });
        }
        for verdict in review.requests {
            let request = state.requests.iter_mut().find(|request| request.id == verdict.request_id)
                .expect("request set validated above");
            request.coverage_verified = verdict.covered && state.tasks.iter()
                .filter(|task| task.request_id == request.id).all(|task| task.status.terminal());
            let owned: Vec<_> = state.tasks.iter().filter(|task| task.request_id == request.id).collect();
            if !verdict.covered && owned.iter().all(|task| task.status.terminal()) {
                let kind = if owned.iter().any(|task| task.kind == TaskKind::Work) {
                    TaskKind::Work
                } else {
                    TaskKind::Answer
                };
                // A review may discover that intake omitted part of the source
                // request. Add that missing acceptance item before execution,
                // without erasing any already verified work or its evidence.
                state.tasks.push(WorkTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    request_id: request.id.clone(),
                    phase: "Acceptance corrections".into(),
                    text: verdict.explanation.clone(),
                    criteria: vec![verdict.explanation],
                    kind,
                    origin: TaskOrigin::User,
                    status: TaskStatus::Pending,
                    reason: None,
                    verification: None,
                });
            }
        }
        state.blocker = None;
        Ok(())
    })?;
    Ok(state)
}
