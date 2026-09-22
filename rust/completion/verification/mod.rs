mod asks;
mod evidence;
mod paths;

use super::{model::*, store};
pub(crate) use evidence::{inspect as inspect_evidence, review_evidence};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod index;

use index::{canonical, EvidenceIndex};

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
        let expected: BTreeSet<_> = state
            .tasks
            .iter()
            .filter(|task| !task.status.terminal() && task.status != TaskStatus::Paused)
            .map(|task| task.id.as_str())
            .collect();
        // A verifier that answers about the only open task and does not repeat
        // its identifier has still answered about it. Real reviews omitted
        // `taskId` on 2026-09-15 and left finished work unverified, so the one
        // unambiguous case is bound here; with more than one task open the
        // verdict must say which, and the refusal lists the identifiers.
        let mut review = review;
        let verdicts = review.tasks.len();
        for verdict in &mut review.tasks {
            if verdict.task_id.is_none() {
                if verdicts == expected.len() && verdicts == usize::from(true) {
                    verdict.task_id = expected.iter().next().map(|id| (*id).to_owned());
                } else {
                    return Err(format!(
                        "a verdict names no task; taskId must be one of {}",
                        expected.iter().copied().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }
        let reported: BTreeSet<_> = review
            .tasks
            .iter()
            .filter_map(|task| task.task_id.as_deref())
            .collect();
        if expected != reported || reported.len() != review.tasks.len() {
            return Err("independent review must cover every open task exactly once".into());
        }
        let expected_requests: BTreeSet<_> = state
            .requests
            .iter()
            .filter(|request| request.planned && !request.coverage_verified && !request.paused)
            .map(|request| request.id.as_str())
            .collect();
        let reported_requests: BTreeSet<_> = review
            .requests
            .iter()
            .map(|request| request.request_id.as_str())
            .collect();
        if expected_requests != reported_requests
            || reported_requests.len() != review.requests.len()
        {
            return Err(
                "independent review must compare every unresolved original user request".into(),
            );
        }
        for verdict in &review.tasks {
            let task = state
                .tasks
                .iter()
                .find(|task| Some(task.id.as_str()) == verdict.task_id.as_deref())
                .expect("task set validated above");
            if verdict.explanation.trim().is_empty()
                || verdict.criteria.len() != task.criteria.len()
            {
                return Err(format!(
                    "review of {} must explain every acceptance criterion",
                    task.id
                ));
            }
            let mut checked = BTreeSet::new();
            for criterion in &verdict.criteria {
                if criterion.index >= task.criteria.len()
                    || !checked.insert(criterion.index)
                    || criterion.explanation.trim().is_empty()
                {
                    return Err(format!(
                        "review of {} contains an invalid or duplicate criterion",
                        task.id
                    ));
                }
                let mut observed = false;
                let mut places = BTreeSet::new();
                for reference in &criterion.evidence {
                    if index.observation(reference)? {
                        observed = true;
                        places.extend(index.places(reference)?);
                    }
                }
                if verdict.status == ReviewStatus::Done
                    && (!criterion.satisfied || (task.kind != TaskKind::Answer && !observed))
                {
                    return Err(format!(
                        "task {} criterion {} has no successful independent observation",
                        task.id, criterion.index
                    ));
                }
                // Reading the criterion's prose is the reviewer's judgement.
                // The place it names is not a judgement, so a verdict that
                // rests on work done somewhere else is refused here.
                if verdict.status == ReviewStatus::Done && task.kind != TaskKind::Answer {
                    let wanted = paths::named(&task.criteria[criterion.index]);
                    let workspace = state
                        .requests
                        .iter()
                        .find(|request| request.id == task.request_id)
                        .map_or_else(|| PathBuf::from("."), |request| PathBuf::from(&request.cwd));
                    if let Some(place) = paths::unmatched(&wanted, &places, &workspace) {
                        return Err(format!(
                            "task {} criterion {} names {place}, and no accepted observation happened there",
                            task.id, criterion.index
                        ));
                    }
                }
            }
            let mut observed_failure = false;
            let mut failed_since_answer = false;
            let answered_at = task
                .operator_request
                .as_ref()
                .and_then(|request| request.answer.as_ref())
                .map(|answer| answer.answered_at.as_str());
            for reference in &verdict.evidence {
                observed_failure |= index.failure(reference)?;
                if let Some(stamp) = answered_at {
                    failed_since_answer |= index.failure_after(reference, stamp)?;
                }
            }
            asks::check(task, verdict, observed_failure, failed_since_answer)?;
        }
        if review
            .requests
            .iter()
            .any(|request| request.explanation.trim().is_empty())
        {
            return Err("request coverage decisions require an explanation".into());
        }
        for verdict in review.tasks {
            let task = state
                .tasks
                .iter_mut()
                .find(|task| Some(task.id.as_str()) == verdict.task_id.as_deref())
                .expect("task set validated above");
            task.reason = Some(verdict.explanation.clone());
            task.status = match verdict.status {
                ReviewStatus::Done => TaskStatus::Done,
                ReviewStatus::Continue => TaskStatus::Pending,
                ReviewStatus::Blocked => TaskStatus::Blocked,
            };
            asks::record(task, &verdict);
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
            let request = state
                .requests
                .iter_mut()
                .find(|request| request.id == verdict.request_id)
                .expect("request set validated above");
            request.coverage_verified = verdict.covered
                && state
                    .tasks
                    .iter()
                    .filter(|task| task.request_id == request.id)
                    .all(|task| task.status.terminal());
            let owned: Vec<_> = state
                .tasks
                .iter()
                .filter(|task| task.request_id == request.id)
                .collect();
            if !verdict.covered && owned.iter().all(|task| task.status.terminal()) {
                let kind = if owned.iter().any(|task| task.kind != TaskKind::Answer) {
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
                    defect_of: None,
                    origin: TaskOrigin::User,
                    status: TaskStatus::Pending,
                    reason: None,
                    verification: None,
                    operator_request: None,
                });
            }
        }
        state.blocker = None;
        Ok(())
    })?;
    Ok(state)
}
