//! A task that waits on the operator says exactly what it waits for, and the
//! operator's answer through `jeden todo answer` is the only thing that moves
//! it on. The model-driven journey below blocks on a value that exists in no
//! file, records the ask, refuses to finish without it, and finishes once the
//! answer is recorded. The refusals below run without a model.

use crate::home::Home;
use serde_json::Value;
use std::fs;

fn waiting_task(state: &Value) -> Option<&Value> {
    state["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["status"] == "blocked" && !task["operatorRequest"]["ask"].is_null())
}

#[test]
fn blocked_task_names_its_ask_and_finishes_with_the_operators_answer() {
    let home = Home::new("operator-answer");
    let prompt = "Create token.txt containing exactly the deployment token I hold. I have not given it to you and it exists in no file, environment variable or command output; never invent one or write a placeholder. Do not run commands, open applications, contact anyone or send notifications.";
    let first = home.run(&["run", prompt, "--allow-write", "--max-steps", "24", "--json"]);
    let state = home.state();
    assert!(state["requests"]
        .as_array()
        .unwrap()
        .iter()
        .any(|request| request["prompt"] == prompt));
    assert!(!home.workspace().join("token.txt").exists());
    let Some(task) = waiting_task(&state) else {
        panic!(
            "Real model did not record an ask on a blocked task; not passed: {}\n{}",
            String::from_utf8_lossy(&first.stderr),
            serde_json::to_string_pretty(&state).unwrap()
        );
    };
    assert!(!first.status.success());
    assert_eq!(home.value(&["todo", "list", "--json"])["status"], "waiting_for_operator");
    let ask = task["operatorRequest"]["ask"].as_str().unwrap();
    let id = task["id"].as_str().unwrap();
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(stderr.contains("waiting_for_operator") && stderr.contains(ask) && stderr.contains(id));
    let listed = home.ok(&["todo", "list"]);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains(&format!("Waiting on you: {ask}")));
    assert!(listed.contains(&format!("jeden todo answer {id}")));

    let resumed = home.run(&["todo", "continue", "--allow-write", "--max-steps", "8", "--json"]);
    assert!(!resumed.status.success());
    assert_eq!(home.value(&["todo", "list", "--json"])["status"], "waiting_for_operator");
    assert!(!home.workspace().join("token.txt").exists());

    let answered = home.value(&["todo", "answer", id, "--revision", &home.state()["revision"].to_string(), "--text", "The deployment token is WISENT-4471.", "--json"]);
    let task = answered["tasks"].as_array().unwrap().iter().find(|task| task["id"] == id).unwrap();
    assert_eq!(task["status"], "pending");
    // A fresh review may word the same ask differently; the answer sits
    // beside whichever wording was open when it was given.
    assert!(task["operatorRequest"]["ask"].as_str().is_some_and(|ask| !ask.is_empty()));
    assert_eq!(task["operatorRequest"]["answer"]["text"], "The deployment token is WISENT-4471.");
    assert_eq!(answered["status"], "working");
    let before = home.state();
    let again = home.run(&["todo", "answer", id, "--revision", &answered["revision"].to_string(), "--text", "A second answer"]);
    assert!(!again.status.success());
    assert_eq!(home.state(), before);

    let finished = home.run(&["todo", "continue", "--allow-write", "--max-steps", "24", "--json"]);
    let state = home.state();
    if !finished.status.success() {
        panic!(
            "Real model did not finish with the recorded answer; not passed: {}\n{}",
            String::from_utf8_lossy(&finished.stderr),
            serde_json::to_string_pretty(&state).unwrap()
        );
    }
    assert_eq!(fs::read(home.workspace().join("token.txt")).unwrap(), b"WISENT-4471");
    let task = state["tasks"].as_array().unwrap().iter().find(|task| task["id"] == id).unwrap();
    assert_eq!(task["status"], "done");
    assert_eq!(task["operatorRequest"]["answer"]["text"], "The deployment token is WISENT-4471.");
    home.passed();
}

#[test]
fn answer_is_refused_where_nothing_was_asked() {
    let home = Home::new("operator-answer-refusals");
    let added = home.value(&["todo", "add", "Create alpha.txt containing ALPHA", "--json"]);
    let request = added["requests"][0]["id"].as_str().unwrap();
    let revision = added["revision"].to_string();
    let before = home.state();
    for (target, revision, text) in [
        (request, revision.as_str(), "ALPHA"),
        (request, revision.as_str(), " "),
        ("missing", revision.as_str(), "ALPHA"),
        (request, "0", "ALPHA"),
    ] {
        let result = home.run(&["todo", "answer", target, "--revision", revision, "--text", text]);
        assert!(!result.status.success(), "{target} {revision} {text:?}");
        assert_eq!(home.state(), before);
    }
    let result = home.run(&["todo", "answer", request, "--revision", &revision]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--text"));
    assert_eq!(home.state(), before);
    home.control(request, "cancel");
    let cancelled = home.state();
    let task = cancelled["tasks"][0]["id"].as_str().unwrap();
    assert_eq!(cancelled["tasks"][0]["status"], "cancelled");
    let result = home.run(&["todo", "answer", task, "--revision", &cancelled["revision"].to_string(), "--text", "ALPHA"]);
    assert!(!result.status.success());
    assert_eq!(home.state(), cancelled);
    home.passed();
}
