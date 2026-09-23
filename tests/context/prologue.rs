//! The automatic half: a turn must start with the locators it would
//! otherwise search for.
//!
//! The last case drives a real `jeden run`, then reads that run's own session
//! ledger. It asserts the recorded request rather than the model's answer,
//! because the prologue is what is under test and it is recorded before the
//! first model call: the case therefore measures the real turn whether or not
//! the route answers.

use crate::fixture::Workspace;
use serde_json::Value;

#[test]
fn the_block_a_turn_appends_carries_the_locator() {
    let workspace = Workspace::new("prologue-block");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    let run = workspace.run(&["context", "prompt", "lease renewal"]);
    assert!(run.success, "prompt failed: {}", run.stderr);
    assert!(
        run.stdout.contains("[Context recommendations]"),
        "the block must be labelled so a reader can tell it from the operator's words: {}",
        run.stdout
    );
    assert!(
        run.stdout.contains("notes/fleet.md:"),
        "the block must carry the locator of the seeded section: {}",
        run.stdout
    );
    assert!(
        run.stdout.contains("ranked matches, not verified answers"),
        "the block must say what it is worth: {}",
        run.stdout
    );
}

#[test]
fn switching_the_advisor_off_removes_the_block() {
    let workspace = Workspace::new("prologue-off");
    workspace.configure(serde_json::json!({"enabled": false, "sources": "files", "roots": "."}));
    let run = workspace.run(&["context", "prompt", "lease renewal", "--json"]);
    assert!(run.success, "prompt failed: {}", run.stderr);
    let report = run.json();
    assert_eq!(report["enabled"], false);
    assert_eq!(
        report["block"],
        Value::Null,
        "a disabled advisor appends nothing: {}",
        report["block"]
    );
    let text = workspace.run(&["context", "prompt", "lease renewal"]);
    assert!(
        text.stdout.contains("context.advisor.enabled is false"),
        "the reason must name the setting that switched it off: {}",
        text.stdout
    );
}

#[test]
fn a_recorded_turn_carries_the_advisory() {
    let workspace = Workspace::new("prologue-turn");
    workspace.configure(serde_json::json!({"sources": "files", "roots": "."}));
    // Intake plans the retained request before the turn itself runs, and it
    // spends a step doing it, so a one-step budget ends the run before the
    // prologue this case measures. Three leaves the turn its own step.
    let run = workspace.run(&["run", "how is a lease renewal done", "--max-steps", "3"]);
    let transcript = workspace.transcript();
    let recorded = transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event.pointer("/payload/type").and_then(Value::as_str) == Some("user"))
        .unwrap_or_else(|| {
            panic!(
                "the run recorded no user event, so the turn never reached its prologue. \
                 This case needs a model route the gateway will serve; `jeden doctor` reports \
                 Brama's state. run success={}\nstdout: {}\nstderr: {}",
                run.success, run.stdout, run.stderr
            )
        });
    let task = recorded
        .pointer("/payload/data/task")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the recorded user event carries no task: {recorded}"));
    assert!(
        task.contains("how is a lease renewal done"),
        "the recorded request must still carry the operator's own words: {task}"
    );
    assert!(
        task.contains("[Context recommendations]") && task.contains("notes/fleet.md:"),
        "the turn must have received the advisory with its locator: {task}"
    );
}
