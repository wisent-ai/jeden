//! Reading one request off the wire and writing one answer back, plus the
//! small checks on what a request carried.
//!
//! Split out of `rpc/server/operations.rs`, which had grown past the module
//! line cap.

use serde_json::{json, Value};
use std::io::BufRead;

pub(crate) fn string_param(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{} must be a non-empty string", key))
}

pub(super) fn text_param(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{} must be a string", key))
}

pub(crate) fn wire_id(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

pub(crate) fn success_response(id: Value, result: Value) -> Value {
    json!({"id": id, "result": result})
}

pub(crate) fn error_response(id: Value, code: &str, message: &str) -> Value {
    json!({"id": id, "error": {"code": code, "message": message}})
}

/// One newline-delimited frame, whole, without its line ending; `None` once
/// the peer has closed. The peer is the local process that started this
/// server over stdio, so the frame is as long as it wrote it.
pub(crate) fn read_frame<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    input.read_until(b'\n', &mut frame).map_err(|error| error.to_string())?;
    if frame.is_empty() {
        return Ok(None);
    }
    while matches!(frame.last(), Some(b'\n' | b'\r')) {
        frame.pop();
    }
    Ok(Some(frame))
}
