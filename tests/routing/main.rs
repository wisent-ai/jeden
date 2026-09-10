//! Real turns through the real `jeden` binary against the real Brama gateway:
//! which route answers a signed agent, what a turn does with an answer that
//! arrives unusable, and what an unreadable catalog is allowed to decide.
//!
//! Each case here was written after a real run lost its work to the shape it
//! measures: a subscription refusal that stopped every Weles browser run on
//! the dedicated host, a truncated intake answer that ended a whole retained
//! assignment, and a `GET /v1/models` timeout that ended an assignment before
//! its request ever reached the gateway.
//!
//! A turn needs the environment the binary needs: `BRAMA_URL`, `BRAMA_TOKEN`,
//! `WISENT_APP_AGENT_ID`, `WISENT_APP_AGENT_AUTH_SECRET`, `JEDEN_MODEL`. The
//! binary resolves the two credentials from Stado when the environment does
//! not carry them; a missing one fails the test by name instead of skipping.
//!
//! Run: `npm run test:routing`, which signs the binaries Cargo built before
//! executing them. Runs keep their state under `target/turn-runs`.

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const TURN_ENV: [&str; 5] = [
    "BRAMA_URL",
    "BRAMA_TOKEN",
    "WISENT_APP_AGENT_ID",
    "WISENT_APP_AGENT_AUTH_SECRET",
    "JEDEN_MODEL",
];

struct Turn {
    root: PathBuf,
    env: Vec<(String, String)>,
}

impl Turn {
    fn new(tag: &str) -> Self {
        let mut env = Vec::new();
        for name in TURN_ENV {
            let value = std::env::var(name).unwrap_or_default();
            assert!(
                !value.trim().is_empty(),
                "{name} is required to drive a real model turn"
            );
            env.push((name.to_string(), value));
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/turn-runs")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).expect("create isolated home");
        fs::create_dir_all(root.join("sessions")).expect("create isolated session root");
        fs::create_dir_all(root.join("workspace")).expect("create workspace");
        Self { root, env }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jeden"));
        command
            .env("HOME", self.root.join("home"))
            .env("JEDEN_SESSION_ROOT", self.root.join("sessions"))
            .env_remove("JEDEN_LANGUAGE")
            .current_dir(self.root.join("workspace"));
        for (name, value) in &self.env {
            command.env(name, value);
        }
        command
    }

    /// One `jeden run --model-only` turn, exactly the call Weles makes for a
    /// browser step, with `overrides` applied last so a case can corrupt one
    /// credential without touching the others.
    fn run(&self, prompt: &str, overrides: &[(&str, &str)]) -> (bool, String, String) {
        let mut command = self.command();
        for (name, value) in overrides {
            command.env(name, value);
        }
        let model = std::env::var("JEDEN_MODEL").unwrap_or_default();
        let output = command
            .args([
                "run",
                prompt,
                "--model-only",
                "--json",
                "--model",
                &model,
                "--max-steps",
                "1",
            ])
            .output()
            .expect("run the jeden binary");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// One complete `jeden run` turn - tools, contract and all - so a case sees
    /// what the turn does with the answer, not only whether a route answered.
    fn task(&self, args: &[&str]) -> (bool, String) {
        let output = self
            .command()
            .args(args)
            .output()
            .expect("run the jeden binary");
        let reported = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(self.root.join("run-output.txt"), &reported).expect("retain the run output");
        (output.status.success(), reported)
    }

    /// Every event of every session this run created, oldest first.
    fn events(&self) -> Vec<Value> {
        let mut sessions: Vec<PathBuf> = fs::read_dir(self.root.join("sessions"))
            .expect("read the isolated session root")
            .map(|entry| entry.expect("session directory entry").path())
            .collect();
        sessions.sort();
        let mut events = Vec::new();
        for session in sessions {
            let Ok(text) = fs::read_to_string(session.join("transcript.jsonl")) else {
                continue;
            };
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                events.push(serde_json::from_str(line).expect("a recorded session event"));
            }
        }
        assert!(!events.is_empty(), "the run recorded no session events");
        events
    }

    /// Recorded corrections for one rule of the shared `contract_violation`.
    fn corrections(&self, rule: &str) -> Vec<Value> {
        self.events()
            .iter()
            .filter(|event| {
                event.pointer("/payload/type").and_then(Value::as_str) == Some("contract_violation")
            })
            .map(|event| event["payload"]["data"].clone())
            .filter(|data| data.get("rule").and_then(Value::as_str) == Some(rule))
            .collect()
    }
}

fn field<'a>(data: &'a Value, name: &str) -> &'a str {
    data.get(name).and_then(Value::as_str).unwrap_or_default()
}

#[test]
fn a_signed_agent_turn_is_answered_even_when_its_subscriptions_are_gone() {
    let turn = Turn::new("subscription-alias");
    let (ok, stdout, stderr) = turn.run("Reply with the single word ready.", &[]);
    assert!(
        ok,
        "the turn was refused: stdout={stdout} stderr={}",
        stderr.trim()
    );
    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("run --json printed no envelope ({error}): {stdout}"));
    assert_eq!(
        envelope.get("ok").and_then(Value::as_bool),
        Some(true),
        "envelope reports a failed turn: {envelope}"
    );
    assert!(
        !field(&envelope, "text").trim().is_empty(),
        "the model answered nothing: {envelope}"
    );
}

#[test]
fn a_bad_agent_signature_is_refused_and_never_downgraded() {
    let turn = Turn::new("bad-signature");
    let (ok, stdout, stderr) = turn.run(
        "Reply with the single word ready.",
        &[(
            "WISENT_APP_AGENT_AUTH_SECRET",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )],
    );
    let reported = format!("{stdout}{stderr}");
    assert!(
        !ok,
        "a request signed with the wrong secret was answered anyway: {reported}"
    );
    assert!(
        reported.contains("model router 401")
            || reported.contains("model router 403")
            || reported.contains("unauthorized")
            || reported.contains("authorization_error"),
        "the refusal did not carry the gateway's own status: {reported}"
    );
}

/// An output budget too small for a complete answer is the deterministic shape
/// of a cut-off answer: the gateway itself reports it incomplete. The turn must
/// ask once for a whole answer, then refuse by naming the budget.
#[test]
fn an_answer_cut_off_by_the_output_budget_is_asked_for_again_before_the_turn_stops() {
    let turn = Turn::new("output-budget");
    let model = std::env::var("JEDEN_MODEL").unwrap_or_default();
    let (ok, reported) = turn.task(&[
        "run",
        "Report the absolute path of the workspace directory this turn runs in.",
        "--json",
        "--model",
        &model,
        "--max-tokens",
        "48",
        "--max-steps",
        "6",
    ]);
    let corrections = turn.corrections("model-answer");
    assert!(
        corrections
            .iter()
            .any(|data| field(data, "outcome") == "requested"),
        "the first cut-off answer ended the turn instead of being asked again: {reported}"
    );
    assert!(
        !ok,
        "a turn whose every answer was cut off reported success: {reported}"
    );
    assert!(
        reported.contains("cut off by the output budget of 48 tokens"),
        "the refusal did not name the budget that cut the answer: {reported}"
    );
    assert!(
        !reported.contains("EOF while parsing"),
        "the refusal still quotes a JSON parser column: {reported}"
    );
    let last = corrections.last().expect("a recorded answer correction");
    assert_eq!(
        field(last, "outcome"),
        "rejected",
        "the exhausted correction budget was not recorded: {last}"
    );
    assert_eq!(
        last.get("cutOff").and_then(Value::as_bool),
        Some(true),
        "the correction does not say the answer was cut off: {last}"
    );
}

/// Discovery turns a model name into a readable refusal; it is not the
/// authority on whether a route works. A gateway whose catalog cannot be read
/// must not end the run before the operator's request reaches it.
#[test]
fn an_unreadable_catalog_does_not_decide_the_run() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a port");
    let endpoint = format!("http://{}", closed.local_addr().expect("the reserved port"));
    drop(closed);
    let turn = Turn::new("unread-catalog");
    let (ok, stdout, stderr) = turn.run(
        "Reply with the single word ready.",
        &[("BRAMA_URL", &endpoint)],
    );
    let reported = format!("{stdout}{stderr}");
    assert!(
        !ok,
        "a turn without any gateway reported success: {reported}"
    );
    assert!(
        reported.contains("the Brama catalog could not be read"),
        "the unread catalog was not reported: {reported}"
    );
    assert!(
        reported.contains("/v1/chat/completions"),
        "the run stopped at discovery instead of the request it was asked to make: {reported}"
    );
}
