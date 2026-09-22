//! A `jeden rpc` that Stado placed parks itself when nothing runs.
//!
//! `stado workload attach jeden-session` holds the kind's reservation for as
//! long as `jeden rpc` lives, and hands it the declared park time as
//! `JEDEN_PARK_AFTER_SECONDS`. On 2026-09-22 eight Jeden Desktop tabs held 16
//! cores and 32 GiB of a 12-core laptop for 26 hours at 0.0% CPU, and the
//! laptop refused every fleet build. The behaviour under test is the one
//! that gives such a hold back: with the setting present, a process with an
//! open session and nothing running sends one `parked` event naming why and
//! which sessions it closed, and exits 0 — the exit Stado's attach waits for
//! before releasing the reservation. A setting that is present but not a
//! whole number of seconds is refused before `ready`, with its own sentence.
//!
//! The real binary runs in an isolated `HOME` and `JEDEN_SESSION_ROOT` under
//! Cargo's target scratch directory, so nothing here touches the operator's
//! sessions. Nothing here keeps a clock: the wait for the park ends when the
//! process says it parked or its stdout closes.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The shortest park time the setting accepts, so the test sees the park as
/// soon as the process first looks at itself.
const PARK_AFTER_SECONDS: &str = "1";

struct Home {
    root: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("rpc-parking-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).expect("create isolated home");
        fs::create_dir_all(root.join("sessions")).expect("create isolated session root");
        Self { root }
    }

    fn rpc(&self, park_after: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jeden"));
        command
            .arg("rpc")
            .env("HOME", self.root.join("home"))
            .env("JEDEN_SESSION_ROOT", self.root.join("sessions"))
            .env("JEDEN_PARK_AFTER_SECONDS", park_after)
            .current_dir(&self.root);
        command
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn an_open_session_with_nothing_running_parks_and_exits_zero() {
    let home = Home::new("idle");
    let mut child = home
        .rpc(PARK_AFTER_SECONDS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn jeden rpc");
    // Held open for the whole test: a closed stdin is the client leaving,
    // which ends the process for a different reason than the one under test.
    let mut stdin = child.stdin.take().expect("rpc stdin");
    let request = json!({
        "id": "open",
        "method": "session/new",
        "params": {"cwd": home.root.display().to_string()}
    });
    writeln!(stdin, "{request}").expect("write session/new");

    let stdout = BufReader::new(child.stdout.take().expect("rpc stdout"));
    let mut frames = Vec::new();
    let mut session_id = None;
    let mut parked = None;
    for line in stdout.lines() {
        let line = line.expect("read an rpc frame");
        let frame: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("rpc wrote a line that is not JSON ({error}): {line}"));
        if frame["id"] == "open" {
            session_id = frame
                .pointer("/result/sessionId")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if frame["type"] == "parked" {
            parked = Some(frame.clone());
        }
        frames.push(frame);
    }
    let status = child.wait().expect("wait for jeden rpc");
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().expect("rpc stderr"), &mut stderr)
        .expect("read rpc stderr");
    drop(stdin);

    let session_id =
        session_id.unwrap_or_else(|| panic!("session/new was not answered: {frames:?}\n{stderr}"));
    let parked = parked
        .unwrap_or_else(|| panic!("the process ended without a parked event: {frames:?}\n{stderr}"));
    assert!(
        status.success(),
        "a parked process must exit 0 so the attach releases its hold: {status}\n{stderr}"
    );
    assert_eq!(parked["reason"], "idle", "{parked}");
    assert_eq!(parked["parkAfterSeconds"], 1, "{parked}");
    assert!(
        parked["quietSeconds"].as_u64().is_some_and(|quiet| quiet >= 1),
        "the process parked before its declared quiet period: {parked}"
    );
    assert_eq!(
        parked["sessions"],
        json!([{"sessionId": session_id, "blocker": null}]),
        "the parked event does not name the session it closed"
    );
    assert!(
        stderr.contains("jeden rpc parked (idle)"),
        "the park left no line for the attach's log: {stderr}"
    );
}

#[test]
fn an_unreadable_park_setting_is_refused_before_ready() {
    let home = Home::new("refused");
    let output = home
        .rpc("soon")
        .stdin(Stdio::null())
        .output()
        .expect("run jeden rpc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "an unreadable park setting was accepted: {stdout}"
    );
    assert!(
        stderr.contains(
            "JEDEN_PARK_AFTER_SECONDS must be a whole number of seconds above zero, got \"soon\""
        ),
        "the refusal does not name the setting and the value: {stderr}"
    );
    assert!(
        !stdout.contains("\"ready\""),
        "the process answered ready before refusing its setting: {stdout}"
    );
}
