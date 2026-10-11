//! `jeden chat-memory`: a session's never-ending chat memory as it stands —
//! the log, the view a turn is given, the summary tree — and the same zoom
//! and date recall the model has. Nothing here writes.

use crate::chat_memory::{dir, session};
use crate::Args;

const USAGE: &str = "jeden chat-memory show <session-id-or-path> [--json] — the log length, the view a turn is given now (its nodes and the messages word for word) and every node of the summary tree
jeden chat-memory zoom <session-id-or-path> <node> [--json] — a node's two halves, or a single message word for word
jeden chat-memory dates <session-id-or-path> <from> <to> [--json] — every message that entered the log between two times, in seconds since 1970";

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let words: Vec<&str> = args.positionals.iter().map(String::as_str).collect();
    let memory = |session: &str| dir(&crate::cli::sessions::session_dir_for(session));
    let answer = match words.as_slice() {
        ["show", session] => session::describe(&memory(session))?,
        ["zoom", session, node] => session::zoom(&memory(session), node)?,
        ["dates", session, from, to] => session::dates(&memory(session), from, to)?,
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

/// The answer for a person: each node as `[id] from..to` and its summary,
/// each message as `#index role at` and its text.
fn text(answer: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(log) = answer.get("log") {
        out.push_str(&format!(
            "log {} holds {} message(s)\n",
            log["path"], log["messages"]
        ));
        out.push_str(&format!(
            "view: {} bytes over the first {} message(s); messages from #{} word for word\n",
            answer["view"]["bytes"], answer["view"]["logLength"], answer["view"]["tailStart"]
        ));
    }
    let nodes = answer
        .pointer("/view/frontier")
        .or_else(|| answer.get("children"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .chain(answer.get("node"));
    for node in nodes {
        out.push_str(&format!(
            "[{}] {}..{}\n{}\n",
            node["id"], node["from"], node["to"], node["summary"]
        ));
    }
    let messages = answer
        .pointer("/view/tail")
        .or_else(|| answer.get("messages"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .chain(answer.get("message"));
    for message in messages {
        out.push_str(&format!(
            "#{} {} {}\n{}\n",
            message["index"], message["role"], message["at"], message["content"]
        ));
    }
    out
}
