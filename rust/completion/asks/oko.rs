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

/// The `oko` to run: the one `JEDEN_OKO_BIN` names, or the one on PATH when
/// it names none. A value that is not text is refused, never replaced.
fn binary() -> Result<String, String> {
    match std::env::var(OKO_BINARY_VARIABLE) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok("oko".to_string()),
        Err(error) => Err(format!("{OKO_BINARY_VARIABLE} cannot be read: {error}")),
    }
}

/// Run `oko` and read its JSON answer, or say why there is none.
fn run(arguments: &[&str]) -> Result<Value, String> {
    let program = binary()?;
    let output = stado_wait::output(Command::new(&program).args(arguments)).map_err(|error| {
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
            arguments.join(" "),
            if said.is_empty() {
                output.status.to_string()
            } else {
                said
            }
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("oko answered unreadable JSON: {error}"))
}

/// One delivery Oko reported, or the row itself when it does not name its
/// channel, outcome and detail.
fn delivery(row: &Value) -> String {
    match (
        row["channel"].as_str(),
        row["outcome"].as_str(),
        row["detail"].as_str(),
    ) {
        (Some(channel), Some(outcome), Some(detail)) => format!("{channel} {outcome}: {detail}"),
        _ => format!("a delivery Oko reported without its channel, outcome and detail: {row}"),
    }
}

/// Put `ask` to the operator through Oko. The time is read first: an ask the
/// clock cannot date is not put, so it is never recorded as asked at the epoch.
pub(crate) fn put(ask: &OperatorAsk) -> Result<OkoAsk, String> {
    let at = crate::agent::now_stamp()
        .map_err(|error| format!("ask {} cannot be put on Oko undated: {error}", ask.id))?;
    let detail = format!(
        "Asked by Jeden in {}. Answer here, or with: jeden asks answer {} --text <answer>",
        ask.workspace, ask.id
    );
    let subject = format!("ask-{}", ask.id);
    Ok(
        match run(&[
            "asks",
            "ask",
            "--from",
            "jeden",
            "--subject",
            &subject,
            "--question",
            &ask.ask,
            "--detail",
            &detail,
        ]) {
            Ok(answer) => {
                let oko_id = answer
                    .pointer("/ask/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let deliveries = answer["deliveries"]
                    .as_array()
                    .map(|rows| rows.iter().map(delivery).collect::<Vec<_>>().join("; "))
                    .filter(|text| !text.is_empty());
                OkoAsk {
                    at,
                    detail: match (&oko_id, deliveries) {
                        (Some(id), Some(deliveries)) => {
                            format!("asked in Oko as {id}: {deliveries}")
                        }
                        (Some(id), None) => {
                            format!(
                                "asked in Oko as {id}; `oko asks show {id}` lists its deliveries"
                            )
                        }
                        (None, _) => "oko asks ask answered without an ask id".to_string(),
                    },
                    oko_id,
                }
            }
            Err(reason) => OkoAsk {
                at,
                oko_id: None,
                detail: reason,
            },
        },
    )
}

/// The answer the operator gave to `oko_id` in Oko, if he did.
pub(crate) fn answered(oko_id: &str) -> Result<Option<String>, String> {
    let shown = run(&["asks", "show", oko_id])?;
    if shown["standing"].as_str() != Some("answered") {
        return Ok(None);
    }
    Ok(shown
        .pointer("/ask/answer")
        .and_then(Value::as_str)
        .map(str::to_string))
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
    let put: Vec<(String, OkoAsk)> = waiting
        .iter()
        .map(|ask| self::put(ask).map(|oko| (ask.id.clone(), oko)))
        .collect::<Result<_, String>>()?;
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
