//! Time to completion: the task contract states it on every surface, the
//! intake records one estimate before execution, an accepted independent
//! review records when the work was really done, and a defect that reopens
//! the work removes that moment again while the first estimate stays.
//!
//! The first two journeys need no model. The third drives a real model
//! through the configured route and fails with the route's own refusal when
//! that route cannot serve it, never passing without the measured run.

use crate::home::Home;
use serde_json::{json, Value};
use std::fs;

const CLAUSE: &str = "Before implementation, estimate how long the task will take and tell the user when it will be done. Do not change that first estimate to fit the result. When the task is done, give the actual time next to the estimate. Jeden records the estimate before execution and measures the actual time itself; where no harness measures it, state both yourself.";

fn text(output: std::process::Output) -> String {
    String::from_utf8(output.stdout).unwrap()
}

fn seconds(value: &Value) -> u64 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn the_contract_states_time_to_completion_wherever_it_is_read() {
    let home = Home::new("timing-contract");
    let rendered = text(home.ok(&["contracts", "render"]));
    assert!(
        rendered.contains(&format!("Time to completion: {CLAUSE}")),
        "{rendered}"
    );

    // Another harness gets the same clause in the rules it appends to every
    // system prompt, which is how an Omp session learns it.
    let appended = home.root.join("APPEND_SYSTEM.md");
    home.ok(&["contracts", "install", "--file", appended.to_str().unwrap()]);
    assert!(fs::read_to_string(&appended)
        .unwrap()
        .contains(&format!("Time to completion: {CLAUSE}")));

    let frames = home.rpc(&[json!({"id":"get", "method":"config/contracts/get", "params":{}})]);
    let contract =
        &frames.iter().find(|frame| frame["id"] == "get").unwrap()["result"]["taskContract"];
    assert_eq!(contract["version"], 2);
    assert_eq!(contract["timeToCompletion"]["title"], "Time to completion");
    assert_eq!(contract["timeToCompletion"]["description"], CLAUSE);
    assert!(contract["instructions"]
        .as_str()
        .unwrap()
        .contains(&format!("Time to completion: {CLAUSE}")));
    assert_eq!(contract["requirements"].as_array().unwrap().len(), 7);

    home.ok(&["config", "set", "ui.language", "pl"]);
    let polish = text(home.ok(&["contracts", "render"]));
    assert!(
        polish.contains("Czas ukończenia: Przed implementacją oszacuj, ile potrwa zadanie"),
        "{polish}"
    );
    home.ok(&["config", "unset", "ui.language"]);
    home.passed();
}

#[test]
fn a_request_has_no_time_until_its_intake_and_an_older_ledger_still_opens() {
    let home = Home::new("timing-unplanned");
    let added = home.value(&[
        "todo",
        "add",
        "Create alpha.txt containing exactly ALPHA.",
        "--json",
    ]);
    assert_eq!(added["timing"], json!([]));
    let request = added["requests"][0]["id"].as_str().unwrap().to_owned();
    let saved = home.state();
    assert_eq!(saved["schemaVersion"], 3);
    assert!(saved["requests"][0].get("estimate").is_none());
    assert!(saved["requests"][0].get("completedAt").is_none());
    let listed = text(home.ok(&["todo", "list"]));
    assert!(
        listed.lines().any(|line| line
            == "  Time to completion: not estimated yet; Jeden records the estimate before execution"),
        "{listed}"
    );

    // A ledger written before this version opens as it was and is carried
    // forward; one written by a newer Jeden is refused, not half-read.
    let file = home.session().join("completion.json");
    let mut older = saved.clone();
    older["schemaVersion"] = json!(2);
    fs::write(&file, serde_json::to_vec_pretty(&older).unwrap()).unwrap();
    let upgraded = home.value(&["todo", "list", "--json"]);
    assert_eq!(upgraded["schemaVersion"], 3);
    assert_eq!(upgraded["requests"], saved["requests"]);
    let mut newer = saved.clone();
    newer["schemaVersion"] = json!(4);
    fs::write(&file, serde_json::to_vec_pretty(&newer).unwrap()).unwrap();
    let refused = home.run(&["todo", "list", "--json"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("unsupported completion state version: 4"));
    fs::write(&file, serde_json::to_vec_pretty(&saved).unwrap()).unwrap();

    let cancelled = home.control(&request, "cancel");
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["timing"], json!([]));
    assert!(home.state()["requests"][0].get("completedAt").is_none());
    let listed = text(home.ok(&["todo", "list"]));
    assert!(
        listed
            .lines()
            .any(|line| line == "  Time to completion: no estimate was recorded"),
        "{listed}"
    );
    home.passed();
}

#[test]
fn a_verified_request_keeps_its_first_estimate_beside_the_measured_time() {
    let home = Home::new("timing-real");
    let prompt = "Create alpha.txt containing exactly ALPHA. Finish only after it exists with that content. Do not run commands, open applications, contact anyone, or send notifications.";
    let output = home.run(&[
        "run",
        prompt,
        "--allow-write",
        "--max-steps",
        "24",
        "--json",
    ]);
    if !output.status.success() {
        assert!(home.state()["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|request| request["prompt"] == prompt));
        panic!(
            "Real model completion failed; not passed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read(home.workspace().join("alpha.txt")).unwrap(),
        b"ALPHA"
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["completion"]["complete"], true);

    let state = home.state();
    let request = &state["requests"][0];
    let minutes = request["estimate"]["minutes"].as_u64().unwrap();
    assert!(minutes > 0);
    let estimated_at = seconds(&request["estimate"]["recordedAt"]);
    let completed_at = seconds(&request["completedAt"]);
    assert!(seconds(&request["capturedAt"]) <= estimated_at);
    assert!(estimated_at <= completed_at);

    let snapshot = home.value(&["todo", "list", "--json"]);
    let timing = &snapshot["timing"][0];
    let elapsed = completed_at - estimated_at;
    assert_eq!(timing["requestId"], request["id"]);
    assert_eq!(timing["estimateMinutes"], minutes);
    assert_eq!(seconds(&timing["dueAt"]), estimated_at + minutes * 60);
    assert_eq!(timing["elapsedSeconds"], elapsed);
    assert_eq!(
        timing["differenceSeconds"],
        elapsed as i64 - (minutes * 60) as i64
    );
    let expected = if elapsed <= minutes * 60 {
        "on_time"
    } else {
        "late"
    };
    assert_eq!(timing["state"], expected);
    let answer = result["text"].as_str().unwrap();
    assert!(
        answer.contains("Time to completion: estimated "),
        "{answer}"
    );
    let listed = text(home.ok(&["todo", "list"]));
    assert!(
        listed.contains("  Time to completion: estimated "),
        "{listed}"
    );
    assert!(listed.contains(" · done in "), "{listed}");

    // A defect says the work was not really done: the completion moment
    // goes, the first estimate stays exactly as it was recorded.
    let task = state["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["kind"] == "work")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.value(&[
        "todo",
        "defect",
        &task,
        "--revision",
        &state["revision"].to_string(),
        "--reason",
        "alpha.txt must be checked again",
        "--json",
    ]);
    let reopened = home.state();
    assert!(reopened["requests"][0].get("completedAt").is_none());
    assert_eq!(reopened["requests"][0]["estimate"], request["estimate"]);
    let snapshot = home.value(&["todo", "list", "--json"]);
    assert_eq!(snapshot["timing"][0]["state"], "open");
    assert_eq!(snapshot["timing"][0]["completedAt"], Value::Null);
    home.passed();
}
