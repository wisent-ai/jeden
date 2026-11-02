//! Where a model request goes when the caller exports no Brama address and
//! Stado is installed: handed whole to `stado capabilities call inference
//! --consumer jeden --json`, which asks the first alternative in Stado's
//! selected order that answers it — Brama, or a statement recorded for that
//! exact request — and prints that answer, or refuses with its own sentence
//! naming every alternative's cause and the command that answers a kept
//! request. Jeden names no alternative and reads no credential for it, so an
//! alternative Stado gains later, or an order the operator selects, is used
//! with no change here.
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

/// The consumer Jeden asks Stado's capabilities as.
const CONSUMER: &str = "jeden";
const CALL: [&str; 6] = [
    "capabilities",
    "call",
    "inference",
    "--consumer",
    CONSUMER,
    "--json",
];

/// The command each request is handed to.
pub(crate) fn operation() -> String {
    format!("stado {}", CALL.join(" "))
}

/// Whether Stado can be run on this machine, or why not.
pub(crate) fn installed() -> Result<(), String> {
    let output = stado_wait::output(
        Command::new("stado")
            .arg("--version")
            .stdin(Stdio::null()),
    )
    .map_err(|error| format!("the Stado CLI could not be started: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`stado --version` ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// The inference alternatives Stado lists, in its selected order, as
/// `stado capabilities inference --json` prints them; the error names the
/// command and what it printed.
pub(crate) fn alternatives() -> Result<Value, String> {
    let operation = "stado capabilities inference --json";
    let output = stado_wait::output(
        Command::new("stado")
            .args(["capabilities", "inference", "--json"])
            .stdin(Stdio::null()),
    )
    .map_err(|error| format!("`{operation}` could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`{operation}` ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let listed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("`{operation}` printed no JSON: {error}"))?;
    match listed["alternatives"].as_array().and_then(|families| {
        families
            .iter()
            .find(|family| family["family"] == "inference")
            .map(|family| family["order"].clone())
    }) {
        Some(order) if order.is_array() => Ok(order),
        _ => Err(format!("`{operation}` lists no inference order: {listed}")),
    }
}

/// What Stado did with one request.
pub(crate) enum Called {
    /// The answer the alternative that answered gave, as its JSON text.
    Answered(String),
    /// Brama refused the request itself with this HTTP status and body; the
    /// same refusal every alternative would give, so it is the request's.
    RefusedByBrama { status: u16, body: String },
    /// No alternative answered: Stado's own sentence.
    Unanswered(String),
}

/// Hand one chat-completions `body` to Stado's inference alternatives.
/// Stado that cannot be run, or prints something that is not its answer or
/// its failure envelope, is the error naming the command.
pub(crate) fn call(body: &str) -> Result<Called, String> {
    let operation = operation();
    let mut child = Command::new("stado")
        .args(CALL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("`{operation}` could not start: {error}"))?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| format!("`{operation}` has no input pipe"))
        .and_then(|mut input| {
            input
                .write_all(body.as_bytes())
                .map_err(|error| format!("cannot hand the request to `{operation}`: {error}"))
        });
    if let Err(mut failure) = written {
        if let Err(error) = child.kill() {
            failure.push_str(&format!("; cannot end it: {error}"));
        }
        if let Err(error) = stado_wait::child_output(child, &operation) {
            failure.push_str(&format!("; cannot collect its end: {error}"));
        }
        return Err(failure);
    }
    let output = stado_wait::child_output(child, &operation)
        .map_err(|error| format!("`{operation}` did not complete: {error}"))?;
    let printed: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "`{operation}` ended {} printing no JSON ({error}): {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;
    if output.status.success() {
        return match printed.get("answer") {
            Some(answer) if !answer.is_null() => Ok(Called::Answered(answer.to_string())),
            _ => Err(format!("`{operation}` printed no answer: {printed}")),
        };
    }
    let details = &printed["details"];
    if details["provider"] == "brama" {
        if let Some(status) = details["status"].as_u64().and_then(|s| u16::try_from(s).ok()) {
            return Ok(Called::RefusedByBrama {
                status,
                body: details["answer"].to_string(),
            });
        }
    }
    let sentence = printed["message"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_owned());
    Ok(Called::Unanswered(format!(
        "no inference alternative answered `{operation}` ({}): {sentence}",
        output.status
    )))
}
