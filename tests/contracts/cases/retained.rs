//! What happens to work the operator already asked for: a new question never
//! erases it, a stale revision and a model-owned `done` are refused, a paused
//! request refuses resumption, a cancellation stays authoritative, and a
//! graphical client can queue requests without running a model at all.

use crate::home::Home;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[test]
fn retained_requests_survive_new_questions_pause_and_session_recovery() {
    let home = Home::new("retained-controls");
    let first = home.value(&[
        "todo",
        "add",
        "Create alpha.txt containing exactly ALPHA.",
        "--json",
    ]);
    let request = first["requests"][0]["id"].as_str().unwrap();
    let paused = home.control(request, "pause");
    assert_eq!(paused["complete"], false);
    let before = home.state();
    let stale = home.run(&[
        "todo",
        "resume",
        request,
        "--revision",
        &first["revision"].to_string(),
        "--reason",
        "Stale control",
    ]);
    assert!(!stale.status.success());
    assert_eq!(home.state(), before);
    let source = home.session();
    let resumed = home.run(&["resume", source.to_str().unwrap()]);
    assert!(!resumed.status.success());
    assert_eq!(
        String::from_utf8(resumed.stderr).unwrap().trim(),
        "Error: Retained work is paused; resume its tasks before continuing."
    );
    assert_ne!(home.session(), source);
    assert_eq!(home.state()["requests"], before["requests"]);
    let added = home.value(&[
        "todo",
        "add",
        "Explain the previous request without cancelling it.",
        "--json",
    ]);
    let second = added["requests"][1]["id"].as_str().unwrap();
    assert_eq!(home.state()["requests"][0], before["requests"][0]);
    let denied = home.run(&["todo", "done", request]);
    assert!(!denied.status.success());
    assert_eq!(home.state()["requests"][0], before["requests"][0]);
    home.control(request, "resume");
    assert_eq!(home.state()["requests"][0]["paused"], false);
    home.control(second, "cancel");
    assert_eq!(home.state()["requests"][0]["coverageVerified"], false);
    let cancelled = home.control(request, "cancel");
    assert_eq!(cancelled["status"], "cancelled");
    let final_state = home.state();
    let denied = home.run(&[
        "todo",
        "resume",
        request,
        "--revision",
        &final_state["revision"].to_string(),
        "--reason",
        "Cannot revive cancelled work",
    ]);
    assert!(!denied.status.success());
    assert_eq!(home.state(), final_state);
    home.passed();
}

#[test]
fn graphical_clients_queue_requests_without_running_a_model() {
    let home = Home::new("queued-rpc");
    let options = json!({"cwd": home.workspace(), "allowWrite": true, "allowCommand": false});
    let frames = home.rpc(&[
        json!({"id":"new", "method":"session/new", "params":{"options":options}}),
        json!({"id":"add-mobile", "method":"session/completion/add", "params":{"sessionId":"session-1", "prompt":"Create queued.txt containing QUEUED."}}),
        json!({"id":"add-desktop", "method":"session/prompt", "params":{"sessionId":"session-1", "requestId":"desktop-add", "prompt":"/todo add \"Explain the queued work without cancelling it.\""}}),
        json!({"id":"empty", "method":"session/completion/add", "params":{"sessionId":"session-1", "prompt":""}}),
        json!({"id":"state", "method":"session/completion/get", "params":{"sessionId":"session-1"}}),
    ]);
    let source = frames.iter().find(|frame| frame["id"] == "new").unwrap()["result"]["sessionPath"]
        .as_str()
        .unwrap();
    let saved: Value =
        serde_json::from_slice(&fs::read(PathBuf::from(source).join("completion.json")).unwrap())
            .unwrap();
    let prompts = saved["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["prompt"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        prompts,
        [
            "Create queued.txt containing QUEUED.",
            "Explain the queued work without cancelling it."
        ]
    );
    assert!(saved["requests"]
        .as_array()
        .unwrap()
        .iter()
        .all(|request| request["planned"] == false));
    assert!(!home.workspace().join("queued.txt").exists());
    let refused = frames.iter().find(|frame| frame["id"] == "empty").unwrap();
    assert_eq!(refused["error"]["code"], "invalid_params");
    let state = frames.iter().find(|frame| frame["id"] == "state").unwrap();
    assert_eq!(state["result"]["completion"]["complete"], false);
    home.passed();
}
