//! `jeden asks`: the operator ask register read and answered from the
//! command line. The RPC methods `asks/list`, `asks/get` and `asks/answer`
//! return the same values.

use super::{AskRegister, AskSource, OperatorAsk};
use crate::cli::invocation::refusal::usage;
use crate::completion::store;
use serde_json::{json, Value};
use std::path::Path;

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
    })
}

pub(crate) fn list_value() -> Result<Value, String> {
    let register = super::read()?;
    let mut asks: Vec<&OperatorAsk> = register.asks.iter().collect();
    asks.sort_by_key(|ask| std::cmp::Reverse(ask.asked_at.parse::<u64>().unwrap_or_default()));
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

fn render_list(value: &Value) -> String {
    let asks = value["asks"].as_array().cloned().unwrap_or_default();
    if asks.is_empty() {
        return format!(
            "Nothing has been asked of you. Register: {}",
            value["register"].as_str().unwrap_or_default()
        );
    }
    let mut lines = Vec::new();
    for ask in &asks {
        lines.push(render_summary(ask));
    }
    lines.join("\n")
}

fn render_summary(ask: &Value) -> String {
    let count = ask["timesAsked"].as_u64().unwrap_or_default() as usize;
    let mut text = format!(
        "{} [{}] asked {} since {}: {}",
        ask["id"].as_str().unwrap_or_default(),
        ask["status"].as_str().unwrap_or_default(),
        super::times(count),
        ask["askedAt"].as_str().unwrap_or_default(),
        ask["ask"].as_str().unwrap_or_default(),
    );
    if let Some(follows) = ask["follows"].as_str() {
        text.push_str(&format!(
            "\n  Asked again after ask {follows} because an operation failed after its answer"
        ));
    }
    match ask["answer"].as_object() {
        Some(answer) => text.push_str(&format!(
            "\n  Your answer ({}): {}",
            answer["answeredAt"].as_str().unwrap_or_default(),
            answer["text"].as_str().unwrap_or_default()
        )),
        None => text.push_str(&format!(
            "\n  Answer with: jeden asks answer {} --text <answer>",
            ask["id"].as_str().unwrap_or_default()
        )),
    }
    text
}

fn render_show(value: &Value) -> String {
    let mut lines = vec![render_summary(value)];
    for place in value["occurrences"].as_array().into_iter().flatten() {
        let mut line = format!(
            "  Asked {} by {} in {}",
            place["askedAt"].as_str().unwrap_or_default(),
            place["source"].as_str().unwrap_or_default(),
            place["sessionPath"].as_str().unwrap_or_default(),
        );
        if let Some(task) = place["taskId"].as_str() {
            line.push_str(&format!(" for task {task}"));
        }
        if let Some(status) = place["taskStatus"].as_str() {
            line.push_str(&format!(" (task {status})"));
        }
        if let Some(state) = place["taskState"].as_str() {
            line.push_str(&format!(" ({state})"));
        }
        if place["answeredFromRegister"] == json!(true) {
            line.push_str(" — answered from the register, not asked again");
        }
        lines.push(line);
        lines.push(format!(
            "    {}",
            place["wording"].as_str().unwrap_or_default()
        ));
    }
    lines.join("\n")
}

fn render_answer(value: &Value) -> String {
    let mut lines = vec![format!(
        "Recorded your answer to ask {}: {}",
        value["ask"]["id"].as_str().unwrap_or_default(),
        value["ask"]["answer"]["text"].as_str().unwrap_or_default()
    )];
    let deliveries = value["deliveries"].as_array().cloned().unwrap_or_default();
    if deliveries.is_empty() {
        lines.push("No task waits on this ask now.".into());
    }
    for delivery in deliveries {
        lines.push(format!(
            "  {} in {}: {} — {}",
            delivery["taskId"].as_str().unwrap_or_default(),
            delivery["sessionPath"].as_str().unwrap_or_default(),
            delivery["outcome"].as_str().unwrap_or_default(),
            delivery["detail"].as_str().unwrap_or_default(),
        ));
    }
    lines.join("\n")
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
    let (value, text_output) = match words.as_slice() {
        [] | ["list"] => {
            let value = list_value()?;
            let rendered = render_list(&value);
            (value, rendered)
        }
        ["show", id] => {
            let value = show_value(id)?;
            let rendered = render_show(&value);
            (value, rendered)
        }
        ["answer", id] => {
            let answer = text.ok_or_else(|| {
                usage(format!(
                    "answer requires --text with what the ask asked for\n{USAGE}"
                ))
            })?;
            let value = answer_value(id, answer)?;
            let rendered = render_answer(&value);
            (value, rendered)
        }
        ["show"] | ["answer"] => {
            return Err(usage(format!("{} requires an ask id\n{USAGE}", words[0])))
        }
        [action, ..] if !matches!(*action, "list" | "show" | "answer") => {
            return Err(usage(format!("unknown asks action: {action}\n{USAGE}")))
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
