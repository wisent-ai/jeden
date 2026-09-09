//! Routing through the real `jeden` binary against the real Brama gateway.
//!
//! Jeden signs every model request as its agent identity, which asks Brama's
//! entitlements router for a subscription. When every subscription bound to
//! that agent is unavailable, Brama refuses with `subscription_unavailable`
//! and the same request, presented with the caller's own bearer, is answered
//! from the alias route table. On 2026-09-06 that refusal stopped every Weles
//! browser run on the dedicated host while the bearer beside it was being
//! served, so the fallback is part of the contract and is measured here.
//!
//! A turn needs the environment the binary needs: `BRAMA_URL`, `BRAMA_TOKEN`,
//! `WISENT_APP_AGENT_ID`, `WISENT_APP_AGENT_AUTH_SECRET`, `JEDEN_MODEL`.
//! `scripts/run-with-stado.sh` exports them on a configured workstation; a
//! missing one fails the test by name instead of skipping it.
//!
//! Run: `cargo test --test routing -- --nocapture`

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
        let root = std::env::temp_dir().join(format!("jeden-routing-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).expect("create isolated home");
        fs::create_dir_all(root.join("sessions")).expect("create isolated session root");
        fs::create_dir_all(root.join("workspace")).expect("create workspace");
        Self { root, env }
    }

    /// One `jeden run --model-only` turn, exactly the call Weles makes for a
    /// browser step, with `overrides` applied last so a case can corrupt one
    /// credential without touching the others.
    fn run(&self, prompt: &str, overrides: &[(&str, &str)]) -> (bool, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jeden"));
        command
            .env("HOME", self.root.join("home"))
            .env("JEDEN_SESSION_ROOT", self.root.join("sessions"))
            .current_dir(self.root.join("workspace"));
        for (name, value) in &self.env {
            command.env(name, value);
        }
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
}

#[test]
fn a_signed_agent_turn_is_answered_even_when_its_subscriptions_are_gone() {
    let turn = Turn::new("subscription-fallback");
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
    let text = envelope
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    assert!(!text.is_empty(), "the model answered nothing: {envelope}");
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
