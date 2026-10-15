//! The never-ending chats over the RPC connection, the same operations as
//! `jeden chat-memory`: `chat-memory/list`, `chat-memory/show {target}`,
//! `chat-memory/zoom {target, id, n}`, `chat-memory/date {target, id}` and
//! `chat-memory/import {chat, session, cwd}`. A target is a chat's name or
//! a session that wrote to a chat.

use crate::chat_memory::{chat, session};
use serde_json::Value;
use std::path::Path;

type Answer = Result<Value, (&'static str, String)>;

fn failed(error: String) -> (&'static str, String) {
    ("chat_memory_error", error)
}

/// A parameter given as text, or a number given as a JSON number; anything
/// else, or nothing, is refused naming the parameter.
fn required(params: &Value, method: &str, name: &str) -> Result<String, (&'static str, String)> {
    match params.get(name) {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        Some(Value::Number(number)) => Ok(number.to_string()),
        _ => Err(("invalid_params", format!("{method} requires {name}"))),
    }
}

pub(super) fn request(method: &str, params: &Value) -> Answer {
    let param = |name: &str| required(params, method, name);
    let chat_of = |target: String| chat::resolve(&target).map_err(failed);
    match method {
        "chat-memory/list" => chat::listing().map_err(failed),
        "chat-memory/show" => session::describe(&chat_of(param("target")?)?).map_err(failed),
        "chat-memory/zoom" => {
            session::zoom(&chat_of(param("target")?)?, &param("id")?, &param("n")?).map_err(failed)
        }
        "chat-memory/date" => {
            session::date(&chat_of(param("target")?)?, &param("id")?).map_err(failed)
        }
        "chat-memory/import" => session::import(
            &param("chat")?,
            &param("session")?,
            Path::new(&param("cwd")?),
        )
        .map_err(failed),
        _ => Err((
            "method_not_found",
            format!("unknown chat-memory method: {method}"),
        )),
    }
}
