//! What a conversation message becomes in the chat log: the user's words,
//! a reply and each of its tool calls, a tool's output clipped to its head
//! and tail.

use serde_json::Value;
use std::num::NonZeroU32;

use super::super::{echo_chars, Entry, Kind};

pub(super) fn is_system(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("system")
}

pub(super) fn is_user(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("user")
}

/// The conversation's own messages: all but its system ones.
pub(super) fn own(messages: &[Value]) -> impl Iterator<Item = &Value> {
    messages.iter().filter(|message| !is_system(message))
}

/// A message's content as text: a string as it is, the text parts of a
/// list joined by newlines, nothing for none.
fn content_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// A tool's output as the log keeps it: whole when it is at most `max`
/// characters, else its head and tail, `max` characters in all, with what
/// was left out named between them.
pub(super) fn clipped(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let head = max >> NonZeroU32::MIN.get();
    let tail = max - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(total - tail).collect();
    format!("{start}\n[{} characters left out]\n{end}", total - max)
}

/// The tool whose result is a subagent's report.
const DELEGATION: &str = "delegate_task";

/// What a delegated task's tool result is: the subagent's report, or the
/// error the tool answered when the delegation failed before a subagent
/// ran (a text that is no job's result).
enum Delegated {
    Report(String),
    Failed,
}

/// A delegated task's result read by its structure: a job's result is the
/// design's `work` message, `[agent job]` and the subagent's final output.
fn delegated_result(text: &str) -> Delegated {
    let parsed: Result<Value, _> = serde_json::from_str(text);
    let job = parsed.as_ref().map(|result| {
        (
            result.pointer("/job/agent").and_then(Value::as_str),
            result.pointer("/job/id").and_then(Value::as_str),
            result.get("delegated"),
        )
    });
    match job {
        Ok((Some(agent), Some(id), Some(output))) => {
            let output = match output {
                Value::String(output) => output.clone(),
                other => other.to_string(),
            };
            Delegated::Report(format!("[{agent} {id}] {}", clipped(&output, echo_chars())))
        }
        Ok(_) | Err(_) => Delegated::Failed,
    }
}

/// The ids of every tool call among `messages` that delegated a task.
pub(super) fn delegations<'a>(messages: impl Iterator<Item = &'a Value>) -> Vec<String> {
    messages
        .filter_map(|message| message.get("tool_calls").and_then(Value::as_array))
        .flatten()
        .filter(|call| call.pointer("/function/name").and_then(Value::as_str) == Some(DELEGATION))
        .filter_map(|call| call.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// The log entries one conversation message becomes, from index `next` on.
/// A tool result answering one of the `delegated` calls is a subagent's
/// report, kept as `work`.
pub(super) fn entries_of(
    message: &Value,
    next: u64,
    date: &str,
    delegated: &[String],
) -> Result<Vec<Entry>, String> {
    let role = message.get("role").and_then(Value::as_str).ok_or_else(|| {
        format!("message {next} of the conversation names no role, so the chat log cannot keep it")
    })?;
    let mut entries = Vec::new();
    let mut push = |kind: Kind, text: String| {
        let i = next + entries.len() as u64;
        entries.push(Entry::new(i, kind, text, date.to_string()));
    };
    match role {
        "user" => push(Kind::User, content_text(message)),
        "assistant" => {
            let text = content_text(message);
            if !text.trim().is_empty() {
                push(Kind::Jeden, text);
            }
            for call in message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let function = call.get("function").ok_or_else(|| {
                    format!("a tool call of message {next} names no function: {call}")
                })?;
                let name = function.get("name").and_then(Value::as_str).ok_or_else(|| {
                    format!("a tool call of message {next} names no tool: {call}")
                })?;
                let arguments = match function.get("arguments") {
                    Some(Value::String(text)) => text.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                push(Kind::Tool, format!("{name} {arguments}"));
            }
        }
        "tool" => {
            let answered = message.get("tool_call_id").and_then(Value::as_str);
            let text = content_text(message);
            let delegation = answered.is_some_and(|id| delegated.iter().any(|call| call == id));
            match delegation.then(|| delegated_result(&text)) {
                Some(Delegated::Report(work)) => push(Kind::Work, work),
                Some(Delegated::Failed) | None => push(Kind::Echo, clipped(&text, echo_chars())),
            }
        }
        other => {
            return Err(format!(
                "message {next} of the conversation has the role `{other}`, which the chat log has no kind for"
            ))
        }
    }
    Ok(entries)
}
