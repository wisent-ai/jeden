//! How much of a model's answer the product reads. The verdicts below are the
//! ones a real verifier returned on 2026-09-15, including the echoed field
//! that used to end a turn with the work already done.

use super::*;
use serde_json::json;

fn verdict() -> String {
    json!({
        "tasks": [{
            "taskId": "0075b831",
            "requestId": "5a8f0d21",
            "status": "done",
            "explanation": "The workspace root holds alpha.txt with exactly ALPHA.",
            "criteria": [{
                "index": usize::default(),
                "satisfied": true,
                "explanation": "read back through the reviewer's own session",
                "evidence": []
            }],
            "evidence": []
        }],
        "requests": [{
            "requestId": "5a8f0d21",
            "covered": true,
            "explanation": "one file, one place"
        }]
    })
    .to_string()
}

#[test]
fn a_verdict_that_echoes_a_field_is_still_read() {
    let review: CompletionReview =
        serde_json::from_str(&verdict()).expect("the verdict is readable");
    assert_eq!(review.tasks.len(), review.requests.len());
    assert_eq!(review.tasks[0].task_id, "0075b831");
    assert_eq!(review.tasks[0].status, ReviewStatus::Done);
    assert!(review.requests[0].covered);
    assert_eq!(review.tasks[0].criteria.len(), review.tasks.len());
}

#[test]
fn a_verdict_that_names_no_task_at_all_is_still_refused() {
    // `taskId`, `task_id`, `task` and `id` are the same identifier under
    // different spellings and are all read. A verdict that names the task
    // nowhere cannot be applied to one, however complete the rest looks.
    let nameless = verdict().replace("\"taskId\"", "\"about\"");
    let error = serde_json::from_str::<CompletionReview>(&nameless)
        .expect_err("a verdict that names no task cannot be applied");
    assert!(
        error.to_string().contains("taskId"),
        "the refusal names the missing field: {error}"
    );
}

#[test]
fn a_verdict_that_names_the_task_as_id_is_still_read() {
    let renamed = verdict().replace("\"taskId\"", "\"id\"");
    let review: CompletionReview =
        serde_json::from_str(&renamed).expect("an identifier is an identifier");
    assert_eq!(review.tasks[0].task_id, "0075b831");
}

#[test]
fn an_unreadable_answer_is_quoted_back_with_its_refusal() {
    let unusable = verdict().replace("\"done\"", "\"finished\"");
    let error = serde_json::from_str::<CompletionReview>(&unusable)
        .expect_err("`finished` is not a verdict");
    let refusal = unreadable("acceptance review", &unusable, &error);
    assert!(
        refusal.contains("the answer reads: "),
        "the refusal quotes the answer: {refusal}"
    );
    assert!(
        refusal.contains("finished"),
        "the quote covers what the parser stopped at: {refusal}"
    );
}

#[test]
fn an_intake_plan_also_tolerates_an_echoed_field() {
    let plan: IntakePlan = serde_json::from_str(
        &json!({"tasks": [{
            "text": "write alpha.txt",
            "criteria": ["the workspace root holds it"],
            "phase": "Delivery"
        }]})
        .to_string(),
    )
    .expect("the plan is readable");
    assert_eq!(plan.tasks[0].kind, TaskKind::Work);
    assert!(plan.cancellations.is_empty());
}
