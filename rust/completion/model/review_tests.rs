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
    assert_eq!(review.tasks[0].task_id.as_deref(), Some("0075b831"));
    assert_eq!(review.tasks[0].status, ReviewStatus::Done);
    assert!(review.requests[0].covered);
    assert_eq!(review.tasks[0].criteria.len(), review.tasks.len());
}

#[test]
fn a_verdict_that_names_no_task_is_read_without_one() {
    // `taskId`, `task_id`, `task` and `id` are the same identifier under
    // different spellings and are all read. A verdict that names the task
    // nowhere is still read, and the controller either binds it to the only
    // open task or refuses it by name; the answer shape no longer decides.
    let nameless = verdict().replace("\"taskId\"", "\"about\"");
    let review: CompletionReview =
        serde_json::from_str(&nameless).expect("the rest of the verdict is usable");
    assert_eq!(review.tasks[0].task_id, None);
    assert_eq!(review.tasks[0].status, ReviewStatus::Done);
}

#[test]
fn a_verdict_that_names_the_task_as_id_is_still_read() {
    let renamed = verdict().replace("\"taskId\"", "\"id\"");
    let review: CompletionReview =
        serde_json::from_str(&renamed).expect("an identifier is an identifier");
    assert_eq!(review.tasks[0].task_id.as_deref(), Some("0075b831"));
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
fn a_request_verdict_filed_under_tasks_is_read_as_a_request_verdict() {
    // Returned by a real verifier on 2026-09-18: the request entry as the
    // last element of `tasks`, with no `requests` array at all.
    let misfiled = json!({
        "tasks": [
            serde_json::from_str::<serde_json::Value>(&verdict()).unwrap()["tasks"][0],
            {"requestId": "5a8f0d21", "covered": false, "explanation": "token.txt was not observed"}
        ]
    })
    .to_string();
    let review: CompletionReview =
        serde_json::from_str(&misfiled).expect("a request verdict is a request verdict anywhere");
    assert_eq!(review.tasks.len(), 1);
    assert_eq!(review.requests.len(), 1);
    assert!(!review.requests[0].covered);
}

#[test]
fn a_requests_array_nested_in_a_task_is_read_beside_it() {
    // The other shape from the same day: `requests` written inside the task
    // object instead of next to `tasks`.
    let mut nested: serde_json::Value = serde_json::from_str(&verdict()).unwrap();
    let requests = nested["requests"].take();
    nested["tasks"][0]["requests"] = requests;
    nested.as_object_mut().unwrap().remove("requests");
    let review: CompletionReview =
        serde_json::from_str(&nested.to_string()).expect("the nested array is still the requests");
    assert_eq!(review.tasks.len(), 1);
    assert_eq!(review.requests.len(), 1);
    assert!(review.requests[0].covered);
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
