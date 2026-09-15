//! Which refusals a wider budget answers. The strings below are the ones real
//! acceptance reviews produced on 2026-09-15.

use super::cut_off;

#[test]
fn an_answer_the_budget_cut_is_asked_again_with_room() {
    assert!(cut_off(
        "model answer stopped mid-JSON after 4850 bytes: a JSON string is never closed; \
         the answer was cut off by the output budget of 16384 tokens"
    ));
    assert!(cut_off(
        "model answer stopped mid-JSON after 2387 bytes: brackets are never closed"
    ));
}

#[test]
fn a_refusal_about_the_verdict_itself_is_not_a_budget_problem() {
    assert!(!cut_off(
        "task a77469d1 criterion 0 names scratch/, and no accepted observation happened there"
    ));
    assert!(!cut_off("invalid acceptance review: missing field `taskId`"));
    assert!(!cut_off(
        "independent review must cover every open task exactly once"
    ));
}
