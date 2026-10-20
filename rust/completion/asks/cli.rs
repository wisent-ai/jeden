//! `jeden asks`: the operator ask register read and answered from the
//! command line. The RPC methods `asks/list`, `asks/get` and `asks/answer`
//! return the same values.

use super::{AskRegister, AskSource, OperatorAsk};
use crate::cli::invocation::refusal::usage;
use crate::completion::store;
use serde_json::{json, Value};
use std::path::Path;

mod render;

pub(crate) const USAGE: &str =
    "jeden asks [list|show <ask-id>|answer <ask-id> --text <answer>] [--json]";

/// `answered`; `waiting` while a task still waits on it; `unanswered` once
/// no task does, though the operator never answered.
fn status(register: &AskRegister, ask: &OperatorAsk) -> &'static str {
    if ask.answer.is_some() {
        "answered"
    } else if places(register, ask)
        .iter()
        .any(|place| place["waiting"] == json!(true))
    {
        "waiting"
    } else {
        "unanswered"
    }
}

/// Every occurrence with the state of its task now, read from its session.
fn places(register: &AskRegister, ask: &OperatorAsk) -> Vec<Value> {
    ask.occurrences
        .iter()
        .map(|occurrence| {
            let mut value = json!(occurrence);
            if occurrence.source != AskSource::Review {
                return value;
            }
            let Some(task_id) = &occurrence.task_id else {
                return value;
            };
            let current = register
                .for_task(&occurrence.session_path, task_id)
                .is_some_and(|current| current.id == ask.id);
            match store::read(Path::new(&occurrence.session_path)) {
                Ok(state) => match state.tasks.iter().find(|task| &task.id == task_id) {
                    Some(task) => {
                        value["taskStatus"] = json!(task.status);
                        value["waiting"] = json!(current && task.waits_for_operator());
                    }
                    None => value["taskState"] = json!("the session no longer holds this task"),
                },
                Err(error) => value["taskState"] = json!(error),
            }
            value
        })
        .collect()
}

fn summary(register: &AskRegister, ask: &OperatorAsk) -> Value {
    let mut sessions: Vec<&str> = Vec::new();
    for occurrence in &ask.occurrences {
        if !sessions.contains(&occurrence.session_path.as_str()) {
            sessions.push(&occurrence.session_path);
        }
    }
    json!({
        "id": ask.id,
        "ask": ask.ask,
        "status": status(register, ask),
        "askedAt": ask.asked_at,
        "timesAsked": ask.occurrences.len(),
        "workspace": ask.workspace,
        "sessions": sessions,
        "follows": ask.follows,
        "answer": ask.answer,
        "oko": ask.oko,
    })
}

pub(crate) fn list_value() -> Result<Value, String> {
    let register = super::read()?;
    let mut asks = Vec::new();
    for ask in &register.asks {
        let stamp = ask.asked_at.parse::<u64>().map_err(|error| {
            format!("ask {} records its time as {:?}, which is not milliseconds since the epoch: {error}", ask.id, ask.asked_at)
        })?;
        asks.push((stamp, ask));
    }
    asks.sort_by_key(|(stamp, _)| std::cmp::Reverse(*stamp));
    let asks: Vec<&OperatorAsk> = asks.into_iter().map(|(_, ask)| ask).collect();
    Ok(json!({
        "register": super::register_path(),
        "revision": register.revision,
        "asks": asks.iter().map(|ask| summary(&register, ask)).collect::<Vec<_>>(),
    }))
}

pub(crate) fn show_value(id: &str) -> Result<Value, String> {
    let register = super::read()?;
    let ask = register
        .find(id)
        .ok_or_else(|| format!("unknown ask: {id}; jeden asks list shows every recorded ask"))?;
    let mut value = summary(&register, ask);
    value["occurrences"] = json!(places(&register, ask));
    Ok(value)
}

pub(crate) fn answer_value(id: &str, text: &str) -> Result<Value, String> {
    let report = super::answer(id, text, None)?;
    let failures = report.failures();
    if !failures.is_empty() {
        return Err(format!(
            "the answer to ask {id} is recorded, but it did not reach {} waiting task(s): {}; jeden todo continue in each session hands it over",
            failures.len(),
            failures
                .iter()
                .map(|delivery| format!("{} in {}: {}", delivery.task_id, delivery.session_path, delivery.detail))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    serde_json::to_value(&report).map_err(|error| error.to_string())
}

/// What `jeden asks` does, by the word that names it.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum AskAction {
    List,
    Show,
    Answer,
}
pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
    let json = args.json;
    let mut text = None;
    let mut words = Vec::new();
    let mut arguments = args.positionals.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--text" => {
                text = Some(
                    arguments
                        .next()
                        .ok_or_else(|| usage(format!("--text requires the answer\n{USAGE}")))?
                        .as_str(),
                )
            }
            flag if flag.starts_with("--") => {
                return Err(usage(format!("unknown asks option: {flag}\n{USAGE}")))
            }
            _ => words.push(argument.as_str()),
        }
    }
    let (action, rest) = match words.split_first() {
        None => (AskAction::List, &[][..]),
        Some((word, rest)) => (
            crate::vocabulary::named::<AskAction>(word).map_err(|cause| {
                usage(format!("unknown asks action: {word} ({cause})\n{USAGE}"))
            })?,
            rest,
        ),
    };
    let (value, text_output) = match (action, rest) {
        (AskAction::List, []) => {
            let value = list_value()?;
            let rendered = render::register(&value)?;
            (value, rendered)
        }
        (AskAction::Show, [id]) => {
            let value = show_value(id)?;
            let rendered = render::show(&value)?;
            (value, rendered)
        }
        (AskAction::Answer, [id]) => {
            let answer = text.ok_or_else(|| {
                usage(format!(
                    "answer requires --text with what the ask asked for\n{USAGE}"
                ))
            })?;
            let value = answer_value(id, answer)?;
            let rendered = render::answer(&value)?;
            (value, rendered)
        }
        (_, []) => {
            return Err(usage(format!(
                "{} requires an ask id\n{USAGE}",
                crate::vocabulary::spelling(action)
            )))
        }
        _ => {
            return Err(usage(format!(
                "too many arguments: {}\n{USAGE}",
                words.join(" ")
            )))
        }
    };
    if json {
        serde_json::to_string_pretty(&value)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string())
    } else {
        Ok(text_output + "\n")
    }
}
