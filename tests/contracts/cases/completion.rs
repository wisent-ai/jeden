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
