//! Putting a waiting ask where the operator looks: one ask in the shared Oko
//! database (`oko asks ask`), delivered on the channels he chose there and
//! shown in Oko Desktop and Oko iOS, and an answer given there read back.
//!
//! Jeden runs without Oko too. A machine with no `oko`, or Oko's own refusal
//! (no channel chosen, nobody signed in), is recorded on the ask with its
//! reason, never as asked; the ask still waits in Jeden's register and
//! `jeden asks answer` answers it.

use super::OperatorAsk;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

/// The program that records an ask in the shared Oko database; a machine
/// whose `oko` is not on PATH names it here.
const OKO_BINARY_VARIABLE: &str = "JEDEN_OKO_BIN";

/// What putting one ask on Oko did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OkoAsk {
    pub at: String,
    /// Oko's id for the ask, when it was asked there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oko_id: Option<String>,
    /// What Oko said: the ask it recorded and its deliveries, or why not.
    pub detail: String,
}

fn binary() -> String {
    std::env::var(OKO_BINARY_VARIABLE)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "oko".to_string())
}

/// Run `oko` and read its JSON answer, or say why there is none.
fn run(arguments: &[&str]) -> Result<Value, String> {
    let program = binary();
    let output = Command::new(&program).args(arguments).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("{program} is not installed on this machine (set {OKO_BINARY_VARIABLE} to its path)")
        } else {
            format!("{program} could not start: {error}")
        }
    })?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "oko {} refused: {}",
            arguments.first().copied().unwrap_or_default(),
            if said.is_empty() { output.status.to_string() } else { said }
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("oko answered unreadable JSON: {error}"))
}

/// Put `ask` to the operator through Oko.
pub(crate) fn put(ask: &OperatorAsk) -> OkoAsk {
    let detail = format!(
        "Asked by Jeden in {}. Answer here, or with: jeden asks answer {} --text <answer>",
        ask.workspace, ask.id
    );
    let subject = format!("ask-{}", ask.id);
    let at = crate::agent::now_stamp();
    match run(&[
        "asks", "ask", "--from", "jeden", "--subject", &subject, "--question", &ask.ask, "--detail", &detail,
    ]) {
        Ok(answer) => {
            let oko_id = answer.pointer("/ask/id").and_then(Value::as_str).map(str::to_string);
            let deliveries = answer["deliveries"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| {
                            format!(
                                "{} {}: {}",
                                row["channel"].as_str().unwrap_or_default(),
                                row["outcome"].as_str().unwrap_or_default(),
                                row["detail"].as_str().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|text| !text.is_empty());
            OkoAsk {
                at,
                detail: match (&oko_id, deliveries) {
                    (Some(id), Some(deliveries)) => format!("asked in Oko as {id}: {deliveries}"),
                    (Some(id), None) => format!("asked in Oko as {id}; `oko asks show {id}` lists its deliveries"),
                    (None, _) => "oko asks ask answered without an ask id".to_string(),
                },
                oko_id,
            }
        }
        Err(reason) => OkoAsk { at, oko_id: None, detail: reason },
    }
}

/// The answer the operator gave to `oko_id` in Oko, if he did.
pub(crate) fn answered(oko_id: &str) -> Result<Option<String>, String> {
    let shown = run(&["asks", "show", oko_id])?;
    if shown["standing"].as_str() != Some("answered") {
        return Ok(None);
    }
    Ok(shown.pointer("/ask/answer").and_then(Value::as_str).map(str::to_string))
}

/// Put each of `ids` that still waits and was never put on Oko there, and
/// record what Oko said on the ask. The asks are put outside the register's
/// lock: Oko is a network call, and every other session waits on that lock.
pub(crate) fn put_waiting(ids: &[String]) -> Result<(), String> {
    let register = super::read()?;
    let waiting: Vec<OperatorAsk> = register
        .asks
        .into_iter()
        .filter(|ask| ids.contains(&ask.id) && ask.answer.is_none() && ask.oko.is_none())
        .collect();
    if waiting.is_empty() {
        return Ok(());
    }
    let put: Vec<(String, OkoAsk)> = waiting.iter().map(|ask| (ask.id.clone(), self::put(ask))).collect();
    super::update(|register| {
        for (id, oko) in &put {
            if let Some(ask) = register.asks.iter_mut().find(|ask| &ask.id == id) {
                ask.oko = Some(oko.clone());
            }
        }
        Ok(())
    })
    .map(|_| ())
}

/// Tell Oko the ask was answered in Jeden, so it stops asking; Oko's answer
/// or refusal in one line.
pub(crate) fn mirror_answer(oko_id: &str, text: &str) -> Result<String, String> {
    run(&["asks", "answer", oko_id, "--text", text, "--on", "jeden"])
        .map(|_| format!("Oko ask {oko_id} answered"))
}
