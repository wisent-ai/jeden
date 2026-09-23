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
    let reopened = home.rpc(&[
        json!({"id":"open", "method":"session/open", "params":{"session":source, "options":options}}),
        json!({"id":"retained", "method":"session/completion/get", "params":{"sessionId":"session-1"}}),
        json!({"id":"add-after-reopen", "method":"session/completion/add", "params":{"sessionId":"session-1", "prompt":"Keep the reopened conversation in its original ledger."}}),
        json!({"id":"missing", "method":"session/open", "params":{"session":home.root.join("missing-session"), "options":options}}),
    ]);
    let opened = reopened.iter().find(|frame| frame["id"] == "open").unwrap();
    assert_eq!(opened["result"]["sessionPath"], source);
    let retained = reopened
        .iter()
        .find(|frame| frame["id"] == "retained")
        .unwrap();
    assert_eq!(
        retained["result"]["completion"]["requests"],
        saved["requests"]
    );
    assert_eq!(
        retained["result"]["completion"]["revision"],
        saved["revision"]
    );
    let updated: Value =
        serde_json::from_slice(&fs::read(PathBuf::from(source).join("completion.json")).unwrap())
            .unwrap();
    assert_eq!(updated["requests"][0], saved["requests"][0]);
    assert_eq!(updated["requests"][1], saved["requests"][1]);
    assert_eq!(
        updated["requests"][2]["prompt"],
        "Keep the reopened conversation in its original ledger."
    );
    assert!(!home.root.join("missing-session").exists());
    let missing = reopened
        .iter()
        .find(|frame| frame["id"] == "missing")
        .unwrap();
    assert_eq!(missing["error"]["code"], "session_error");
    home.passed();
}

#[test]
fn import_preserves_history_refreshes_and_protects_adopted_work() {
    let home = Home::new("import");
    let transcripts = home.root.join("transcripts");
    fs::create_dir_all(transcripts.join("nested")).unwrap();
    let source = transcripts.join("nested/source.jsonl");
    // Not a transcript: a directory scan steps over it, an explicit path is refused.
    let stray = transcripts.join("notes.txt");
    fs::write(&stray, "not a transcript\n").unwrap();
    let records = [
        json!({"type":"title","title":"Interrupted work"}),
        json!({"type":"session","id":"import-journey","cwd":home.workspace()}),
        json!({"type":"message","id":"user","parentId":null,"message":{"role":"user","content":"Keep this interrupted request."}}),
        json!({"type":"message","id":"tool","parentId":"user","message":{"role":"assistant","content":[{"type":"toolCall","name":"read","arguments":{"path":"notes.txt"}}]}}),
    ];
    let mut bytes = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&source, &bytes).unwrap();
    let usage = home.run(&["import"]);
    assert!(!usage.status.success());
    assert!(String::from_utf8_lossy(&usage.stderr)
        .contains("Usage: jeden import <path>... [--refresh] [--json]"));
    let refused = home.value(&["import", stray.to_str().unwrap(), "--json"]);
    assert_eq!(
        refused["failures"][0]["error"],
        format!(
            "{}: not a transcript of a supported harness (supported: omp)",
            stray.display()
        )
    );
    assert_eq!(refused["imported"], 0);
    let imported = home.value(&["import", transcripts.to_str().unwrap(), "--json"]);
    assert_eq!(imported["failures"], json!([]));
    assert_eq!(imported["imported"], 1);
    assert_eq!(imported["sessions"][0]["format"], "omp");
    assert_eq!(imported["sessions"][0]["title"], "Interrupted work");
    let destination = PathBuf::from(imported["sessions"][0]["sessionPath"].as_str().unwrap());
    assert_eq!(
        fs::read(destination.join("artifacts/omp-source.jsonl")).unwrap(),
        bytes.as_bytes()
    );
    let ledger = fs::read(destination.join("transcript.jsonl")).unwrap();
    let event: Value = serde_json::from_slice(&ledger).unwrap();
    assert_eq!(
        event["payload"]["data"]["messages"][0]["content"],
        "Keep this interrupted request."
    );
    let state: Value =
        serde_json::from_slice(&fs::read(destination.join("completion.json")).unwrap()).unwrap();
    assert_eq!(
        state["requests"][0]["prompt"],
        "Keep this interrupted request."
    );
    let repeated = home.value(&["import", source.to_str().unwrap(), "--json"]);
    assert_eq!(repeated["existing"], 1);
    assert_eq!(
        fs::read(destination.join("transcript.jsonl")).unwrap(),
        ledger
    );
    bytes.push_str(&json!({"type":"message","id":"next","parentId":"tool","message":{"role":"user","content":"Retain this additional request too."}}).to_string());
    bytes.push('\n');
    fs::write(&source, &bytes).unwrap();
    let changed = home.value(&["import", source.to_str().unwrap(), "--json"]);
    assert_eq!(
        changed["failures"][0]["error"],
        "source changed since import; use --refresh only for a never-adopted import"
    );
    assert_eq!(
        fs::read(destination.join("transcript.jsonl")).unwrap(),
        ledger
    );
    let refreshed = home.value(&["import", source.to_str().unwrap(), "--refresh", "--json"]);
    assert_eq!(refreshed["failures"], json!([]));
    assert_eq!(
        fs::read(destination.join("artifacts/omp-source.jsonl")).unwrap(),
        bytes.as_bytes()
    );
    let state: Value =
        serde_json::from_slice(&fs::read(destination.join("completion.json")).unwrap()).unwrap();
    assert_eq!(
        state["requests"][1]["prompt"],
        "Retain this additional request too."
    );
    let frames = home.rpc(&[
        json!({"id":"open","method":"session/open","params":{"session":destination,"options":{"cwd":home.workspace()}}}),
        json!({"id":"refresh","method":"session/import","params":{"paths":[source],"refresh":true}}),
        json!({"id":"empty","method":"session/import","params":{}}),
    ]);
    let opened = frames.iter().find(|f| f["id"] == "open").unwrap();
    assert_eq!(
        opened["result"]["sessionPath"],
        destination.to_str().unwrap()
    );
    let refused = frames.iter().find(|f| f["id"] == "refresh").unwrap();
    assert_eq!(
        refused["result"]["failures"][0]["error"],
        "native session has been adopted; refusing import refresh"
    );
    let empty = frames.iter().find(|f| f["id"] == "empty").unwrap();
    assert_eq!(empty["error"]["code"], "invalid_params");
    let after: Value =
        serde_json::from_slice(&fs::read(destination.join("completion.json")).unwrap()).unwrap();
    assert_eq!(after["requests"], state["requests"]);
    assert_eq!(fs::read(&source).unwrap(), bytes.as_bytes());
    home.passed();
}
