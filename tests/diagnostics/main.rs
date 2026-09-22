//! A startup refusal against a real unavailable dependency is kept where an
//! operator will look for it: the session ledger records a `run_error` naming
//! the failed operation, the same message reaches standard error, and the
//! completion state stays incomplete.
//!
//! It needs a model route whose startup dependency is really unavailable on
//! the host running it, which only that host can name, so it runs on request:
//! `JEDEN_STARTUP_REFUSAL_MODEL=<route> cargo test --test diagnostics -- --ignored`
//! (`JEDEN_TEST_BINARY` selects another candidate than the one this build
//! produced). Evidence is kept in `target/startup-diagnostics/<id>/report.json`.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn git(arguments: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root())
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {arguments:?} failed");
    output.stdout
}

#[test]
#[ignore = "needs a model route whose startup dependency is unavailable on this host"]
fn startup_refusal_is_persisted() {
    let model = env::var("JEDEN_STARTUP_REFUSAL_MODEL")
        .ok()
        .filter(|value| !value.is_empty())
        .expect("JEDEN_STARTUP_REFUSAL_MODEL names the route whose dependency is unavailable");
    let binary = match env::var("JEDEN_TEST_BINARY") {
        Ok(binary) if !binary.is_empty() => PathBuf::from(binary),
        _ => PathBuf::from(env!("CARGO_BIN_EXE_jeden")),
    };
    let binary = fs::canonicalize(&binary).expect("the Jeden candidate exists");
    let report = root()
        .join("target/startup-diagnostics")
        .join(uuid::Uuid::new_v4().to_string());
    let workspace = report.join("workspace");
    let sessions = report.join("sessions");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::write(
        report.join("source.patch"),
        git(&["diff", "--binary", "HEAD"]),
    )
    .expect("write patch");
    let revision = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"]))
        .trim()
        .to_string();
    let workspace_text = workspace.display().to_string();
    let argv = [
        "run",
        "Read the current directory without changing files.",
        "--cwd",
        workspace_text.as_str(),
        "--model",
        model.as_str(),
    ];
    let mut evidence = json!({
        "source_revision": revision,
        "binary_sha256": format!("{:x}", Sha256::digest(fs::read(&binary).expect("read binary"))),
        "command": argv,
        "status": "failed",
    });
    println!("Evidence: {}", report.display());
    let output = Command::new(&binary)
        .args(argv)
        .env("JEDEN_SESSION_ROOT", &sessions)
        .current_dir(root())
        .output()
        .expect("run the candidate");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    fs::write(report.join("stdout"), &output.stdout).expect("write stdout");
    fs::write(report.join("stderr"), &stderr).expect("write stderr");
    evidence["exit_code"] = json!(output.status.code());
    let save = |evidence: &Value| {
        let text = serde_json::to_string_pretty(evidence).expect("encode report") + "\n";
        fs::write(report.join("report.json"), text).expect("write report");
    };
    save(&evidence);
    assert!(
        !output.status.success(),
        "the refusal needs a real unavailable startup dependency"
    );
    let mut events = Vec::new();
    for session in fs::read_dir(&sessions).expect("sessions were recorded") {
        let transcript = session
            .expect("session entry")
            .path()
            .join("transcript.jsonl");
        let Ok(text) = fs::read_to_string(&transcript) else {
            continue;
        };
        for line in text.lines().filter(|line| !line.is_empty()) {
            events.push(serde_json::from_str::<Value>(line).expect("ledger line is JSON"));
        }
    }
    let failure = events
        .iter()
        .rev()
        .find(|event| event["payload"]["type"] == "run_error")
        .map(|event| event["payload"]["data"].clone())
        .expect("the startup refusal was not retained");
    assert_eq!(failure["operation"], "prepare_turn", "{failure}");
    let message = failure["message"]
        .as_str()
        .expect("the refusal carries a message");
    assert!(stderr.contains(message), "{failure}");
    assert!(events.iter().any(|event| {
        event["payload"]["type"] == "completion_state"
            && event["payload"]["data"]["complete"] == false
    }));
    evidence["status"] = json!("passed");
    evidence["observed_failure"] = failure;
    save(&evidence);
}
