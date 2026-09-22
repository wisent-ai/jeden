//! The cases here are the real ones this rule was written after: receipts in
//! the shape the session ledger records, and criteria in the wording intake
//! produced for them.

use super::*;
use serde_json::json;

fn workspace() -> PathBuf {
    PathBuf::from("/Users/operator/jeden/target/unread-probe/workspace")
}

fn wrote(path: &str, bytes: &str) -> Value {
    json!({
        "tool": "write",
        "input": {"file_path": path, "content": "ALPHA"},
        "result": {"ok": true, "text": format!("Successfully wrote {bytes} bytes to {path}")}
    })
}

#[test]
fn a_criterion_about_the_workspace_root_is_not_met_one_directory_down() {
    let criterion = "The file `alpha.txt` in the workspace root contains exactly ALPHA.";
    let observed = touched(&wrote("workspace/alpha.txt", "five"));
    assert_eq!(
        unmatched(&named(criterion), &observed, &workspace()),
        Some("alpha.txt".to_owned()),
        "the write landed under the root, so the criterion is unmet"
    );
}

#[test]
fn the_same_criterion_is_met_at_the_root_itself() {
    let criterion = "The file `alpha.txt` in the workspace root contains exactly ALPHA.";
    let at_the_root = workspace().join("alpha.txt");
    let observed = touched(&wrote(&at_the_root.display().to_string(), "five"));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_nested_copy_of_the_named_path_is_not_the_named_path() {
    let criterion = "`workspace/alpha.txt` holds the payload.";
    let nested = workspace().join("workspace/workspace").join("alpha.txt");
    let observed = touched(&wrote(&nested.display().to_string(), "five"));
    assert_eq!(
        unmatched(&named(criterion), &observed, &workspace()),
        Some("workspace/alpha.txt".to_owned()),
        "a path ending in the same parts one directory deeper is a different place"
    );
}

#[test]
fn the_path_of_several_parts_is_met_where_the_request_recorded_it() {
    let criterion = "`workspace/alpha.txt` holds the payload.";
    let named_place = workspace().join("workspace/alpha.txt");
    let observed = touched(&wrote(&named_place.display().to_string(), "five"));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_path_of_several_parts_is_met_from_another_root() {
    let criterion = "`rust/completion/verification/paths.rs` carries the rule.";
    let observed = touched(&json!({
        "tool": "read",
        "input": {"path": "/Users/operator/checkout/rust/completion/verification/paths.rs"},
        "result": {"ok": true}
    }));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_neighbouring_file_does_not_meet_it() {
    let criterion = "`rust/completion/verification/paths.rs` carries the rule.";
    let observed = touched(&json!({
        "tool": "read",
        "input": {"path": "/Users/operator/checkout/rust/completion/verification/evidence.rs"},
        "result": {"ok": true}
    }));
    assert_eq!(
        unmatched(&named(criterion), &observed, &workspace()),
        Some("rust/completion/verification/paths.rs".to_owned())
    );
}

#[test]
fn a_published_page_is_met_by_reading_that_page() {
    let criterion = "https://jeden.wisent.com/docs/cli/copy serves the copy verb.";
    let observed = touched(&json!({
        "tool": "bash",
        "input": {"command": "curl --silent https://jeden.wisent.com/docs/cli/copy"},
        "result": {"ok": true}
    }));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_gateway_route_is_met_by_the_call_that_used_it() {
    let criterion = "The turn reaches 127.0.0.1:17601/v1/models before answering.";
    let observed = touched(&json!({
        "tool": "model",
        "result": {"ok": true, "endpoint": "http://127.0.0.1:17601/v1/models"}
    }));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_directory_criterion_is_met_by_a_file_inside_it() {
    let criterion = "The suite lives under tests/contracts/.";
    let observed = touched(&json!({
        "tool": "write",
        "input": {"file_path": "tests/contracts/cases/completion.rs"}
    }));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_run_workspace_named_without_a_trailing_slash_is_still_a_place() {
    // The real contract journey wrote its criterion this way on 2026-09-11,
    // and demanding an observation at the directory itself refused a file
    // that had landed inside it.
    let criterion = format!(
        "The file exists in {} with the exact contents ALPHA.",
        workspace().display()
    );
    let observed = touched(&wrote(
        &workspace().join("alpha.txt").display().to_string(),
        "five",
    ));
    assert_eq!(unmatched(&named(&criterion), &observed, &workspace()), None);
}

#[test]
fn a_file_criterion_stays_strict_when_the_place_is_a_directory() {
    let criterion = "alpha.txt in the workspace root contains ALPHA.";
    let observed = touched(&wrote("sub/alpha.txt", "five"));
    assert_eq!(
        unmatched(&named(criterion), &observed, &workspace()),
        Some("alpha.txt".to_owned())
    );
}

#[test]
fn a_place_quoted_at_the_end_of_a_sentence_keeps_no_punctuation() {
    // The real journey of 2026-09-15 recorded this criterion and the rule read
    // the place as ``scratch/` ``, which nothing could ever match, so the run
    // spent its budget on a place that did not exist.
    let criterion = "A copy already sits in `scratch/`.";
    assert_eq!(
        named(criterion).into_iter().collect::<Vec<_>>(),
        vec!["scratch/".to_owned()]
    );
    let observed = touched(&wrote("scratch/alpha.txt", "five"));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_hidden_directory_keeps_its_leading_dot() {
    let criterion = "The state lives in `.jeden/probe.txt`.";
    assert_eq!(
        named(criterion).into_iter().collect::<Vec<_>>(),
        vec![".jeden/probe.txt".to_owned()]
    );
}

#[test]
fn prose_with_a_slash_in_it_names_no_place() {
    let criterion = "The answer explains the input/output split and the CLI/GUI parity.";
    assert!(
        named(criterion).is_empty(),
        "prose is not a place: {:?}",
        named(criterion)
    );
}

#[test]
fn a_place_the_sentence_only_mentions_does_not_have_to_be_observed() {
    // The criterion a real intake wrote on 2026-09-15. `scratch/` is where the
    // old copy sits; the root is what the request asks for, and demanding both
    // left the run refusing its own finished work.
    let criterion =
        "alpha.txt in the workspace root contains exactly ALPHA; a copy already sits in `scratch/`.";
    let observed = touched(&wrote(
        &workspace().join("alpha.txt").display().to_string(),
        "five",
    ));
    assert_eq!(unmatched(&named(criterion), &observed, &workspace()), None);
}

#[test]
fn a_verdict_anchored_at_no_named_place_is_still_refused() {
    let criterion =
        "alpha.txt in the workspace root contains exactly ALPHA; a copy already sits in `scratch/`.";
    let observed = touched(&wrote("elsewhere/alpha.txt", "five"));
    assert_eq!(
        unmatched(&named(criterion), &observed, &workspace()),
        Some("alpha.txt".to_owned())
    );
}

#[test]
fn prose_versions_and_hosts_name_no_place() {
    let criterion = "jeden 0.1.1 answered through brama.wisent.com at 127.0.0.1 in 1.5 seconds.";
    assert!(
        named(criterion).is_empty(),
        "nothing here is a path: {:?}",
        named(criterion)
    );
}

#[test]
fn a_criterion_that_names_nothing_leaves_the_prose_to_the_reviewer() {
    let criterion = "The answer says which route served the turn.";
    assert!(named(criterion).is_empty());
    assert_eq!(
        unmatched(&named(criterion), &BTreeSet::new(), &workspace()),
        None
    );
}
