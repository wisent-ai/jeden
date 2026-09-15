//! The delivery journey a real model drives: the files it was asked for exist
//! with the exact contents, every recorded verification reference is an event
//! a retained transcript really holds, and resuming the finished session
//! repeats none of its effects. An unavailable route fails this journey with
//! the provider's own refusal rather than passing.

use crate::home::Home;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[test]
fn task_contract_delivery_observes_real_files_and_retains_results_on_resume() {
    let home = Home::new("real-completion");
    let prompt = "Create alpha.txt containing exactly ALPHA and beta.txt containing exactly BETA. Finish only after both exist with those contents. Do not run commands, open applications, contact anyone, or send notifications.";
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
        assert!(
            !home.workspace().join("alpha.txt").exists()
                || home.state()["requests"][0]["coverageVerified"] == false
        );
        panic!(
            "Real model completion failed; not passed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read(home.workspace().join("alpha.txt")).unwrap(),
        b"ALPHA"
    );
    assert_eq!(
        fs::read(home.workspace().join("beta.txt")).unwrap(),
        b"BETA"
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["completion"]["complete"], true);
    for task in home.state()["tasks"].as_array().unwrap() {
        assert_eq!(task["status"], "done");
        for reference in task["verification"]["evidence"].as_array().unwrap() {
            let session = PathBuf::from(reference["sessionPath"].as_str().unwrap());
            let events = fs::read_to_string(session.join("transcript.jsonl")).unwrap();
            assert!(events
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .any(|event| event["eventId"] == reference["eventId"]));
        }
    }
    let before = home.state()["requests"].clone();
    let modification = fs::metadata(home.workspace().join("alpha.txt"))
        .unwrap()
        .modified()
        .unwrap();
    home.ok(&["resume", home.session().to_str().unwrap()]);
    assert_eq!(home.state()["requests"], before);
    assert_eq!(
        fs::metadata(home.workspace().join("alpha.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modification
    );
    home.passed();
}

/// The trap the place rule exists for. A copy of the file already sits one
/// directory below the place the request names, so observing that copy is the
/// cheapest way to call the work done, and on 2026-09-10 that is exactly how a
/// run was accepted. The turn may answer only once the named place itself
/// holds the file; any refusal on the way is retained with its reason.
#[test]
fn a_copy_below_the_named_place_does_not_finish_the_work() {
    let home = Home::new("named-place");
    fs::create_dir_all(home.workspace().join("scratch")).unwrap();
    fs::write(home.workspace().join("scratch/alpha.txt"), b"ALPHA").unwrap();
    let prompt = "The workspace root must hold alpha.txt containing exactly ALPHA. A copy already sits in scratch/alpha.txt. Finish only after the workspace root itself holds that file. Do not run commands, open applications, contact anyone, or send notifications.";
    let output = home.run(&[
        "run",
        prompt,
        "--allow-write",
        "--max-steps",
        "24",
        "--json",
    ]);
    for refusal in refusals(&home) {
        println!("retained refusal: {refusal}");
    }
    let named = home.workspace().join("alpha.txt");
    if !output.status.success() {
        assert!(!named.exists() || home.state()["requests"][0]["coverageVerified"] == false);
        panic!(
            "Real model completion failed; not passed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(fs::read(&named).unwrap(), b"ALPHA");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["completion"]["complete"], true);
    for task in home.state()["tasks"].as_array().unwrap() {
        assert_eq!(task["status"], "done");
    }
    home.passed();
}

/// Every verification refusal the session retained, in order, with the reason
/// the controller gave for it.
fn refusals(home: &Home) -> Vec<String> {
    let transcript = home.session().join("transcript.jsonl");
    fs::read_to_string(transcript)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["payload"]["type"] == "completion_rejected")
        .filter_map(|event| {
            event["payload"]["data"]["reason"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}
