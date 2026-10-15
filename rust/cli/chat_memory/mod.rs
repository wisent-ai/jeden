//! `jeden chat-memory`: the never-ending chats as they stand — which chats
//! there are, a chat's log, the view a turn is given and the summary tree,
//! the same zoom and date the model has — and bringing an earlier session
//! into a chat. Only `import` writes.

use serde_json::Value;

use crate::chat_memory::{chat, session};
use crate::Args;

const USAGE: &str = "jeden chat-memory list [--json] — every chat: its name, the workspace it was started in, how many messages it holds and whether a process writes it now
jeden chat-memory show <chat-or-session> [--json] — a chat (by name, or the chat a session wrote to): how many messages its log holds, the view a turn is given now (its lines and its size between the design's bounds) and every line of the summary tree
jeden chat-memory zoom <chat-or-session> <id> <n> [--json] — line id+n opened into the two lines it was made from, or for n = 1 the message whole
jeden chat-memory date <chat-or-session> <id> [--json] — the date and time of message id
jeden chat-memory import <chat> <session> [--json] — every message of an earlier session appended to the chat as a note, once per session";

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let words: Vec<&str> = args.positionals.iter().map(String::as_str).collect();
    let answer = match words.as_slice() {
        ["list"] => chat::listing()?,
        ["show", target] => session::describe(&chat::resolve(target)?)?,
        ["zoom", target, id, n] => session::zoom(&chat::resolve(target)?, id, n)?,
        ["date", target, id] => session::date(&chat::resolve(target)?, id)?,
        ["import", name, source] => session::import(name, source, &args.cwd)?,
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

/// One chat as `list` and `show` name it.
fn chat_line(chat: &Value) -> String {
    let writer = match chat["writtenNow"].as_bool() {
        Some(true) => "written by a running process",
        Some(false) | None => "not written now",
    };
    format!(
        "{}: {} message(s), started in {} on {}, {writer}\n",
        plain(&chat["name"]),
        chat["messages"],
        plain(&chat["workspace"]),
        plain(&chat["started"]),
    )
}

/// The answer for a person: the chats; a chat's log and view sizes and the
/// view's lines; a zoom's lines or its message as `#i kind date` and its
/// text; a date; an import.
fn text(answer: &Value) -> String {
    let mut out = String::new();
    for chat in answer["chats"].as_array().into_iter().flatten() {
        out.push_str(&chat_line(chat));
    }
    if let Some(log) = answer.get("log") {
        let view = &answer["view"];
        out.push_str(&chat_line(&answer["chat"]));
        out.push_str(&format!(
            "log {} holds {} message(s)\nview: {} bytes over the first {} message(s), merged down to {} once past {}; {} compaction(s) at once\n{}",
            plain(&log["directory"]),
            log["messages"],
            view["bytes"],
            view["covers"],
            view["low"],
            view["high"],
            answer["compactionCalls"],
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
    if let (Some(session), Some(first)) = (answer.get("session"), answer.get("first")) {
        out.push_str(&format!(
            "imported {} message(s) of {} into {} as notes, from message {first}\n",
            answer["messages"],
            plain(session),
            plain(&answer["chat"])
        ));
    }
    out
}
