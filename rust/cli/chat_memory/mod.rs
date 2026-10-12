//! `jeden chat-memory`: a session's never-ending chat memory as it stands —
//! the log, the view a turn is given, the summary tree — and the same zoom
//! and date the model has. Nothing here writes.

use serde_json::Value;

use crate::chat_memory::{dir, session};
use crate::Args;

const USAGE: &str = "jeden chat-memory show <session-id-or-path> [--json] — how many messages the log holds, the view a turn is given now (its lines and its size between the design's bounds) and every line of the summary tree
jeden chat-memory zoom <session-id-or-path> <id> <n> [--json] — line id+n opened into the two lines it was made from, or for n = 1 the message whole
jeden chat-memory date <session-id-or-path> <id> [--json] — the date and time of message id";

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let words: Vec<&str> = args.positionals.iter().map(String::as_str).collect();
    let memory = |session: &str| dir(&crate::cli::sessions::session_dir_for(session));
    let answer = match words.as_slice() {
        ["show", session] => session::describe(&memory(session))?,
        ["zoom", session, id, n] => session::zoom(&memory(session), id, n)?,
        ["date", session, id] => session::date(&memory(session), id)?,
        _ => {
            return Err(crate::cli::invocation::refusal::usage(format!(
                "jeden chat-memory takes one of:\n{USAGE}"
            )))
        }
    };
    if args.json {
        return serde_json::to_string_pretty(&answer)
            .map(|text| text + "\n")
            .map_err(|error| format!("the chat memory cannot be printed as JSON: {error}"));
    }
    Ok(text(&answer))
}

/// A value as a person reads it: a string as it is, anything else as JSON.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The answer for a person: the log and view sizes and the view's lines;
/// a zoom's lines or its message as `#i kind date` and its text; a date.
fn text(answer: &Value) -> String {
    let mut out = String::new();
    if let Some(log) = answer.get("log") {
        let view = &answer["view"];
        out.push_str(&format!(
            "log {} holds {} message(s)\nview: {} bytes over the first {} message(s), merged down to {} once past {}\n{}",
            plain(&log["directory"]),
            log["messages"],
            view["bytes"],
            view["covers"],
            view["low"],
            view["high"],
            plain(&view["text"])
        ));
    }
    for line in answer["lines"].as_array().into_iter().flatten() {
        out.push_str(&plain(line));
    }
    if let Some(message) = answer.get("message") {
        out.push_str(&format!(
            "#{} {} {}\n{}\n",
            message["i"],
            plain(&message["kind"]),
            plain(&message["date"]),
            plain(&message["text"])
        ));
    }
    if let (Some(id), Some(date)) = (answer.get("id"), answer.get("date")) {
        out.push_str(&format!("#{id} {}\n", plain(date)));
    }
    out
}
