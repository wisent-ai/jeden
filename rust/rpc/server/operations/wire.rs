//! Reading one request off the wire and writing one answer back, plus the
//! small checks on what a request carried.
//!
//! Split out of `rpc/server/operations.rs`, which had grown past the module
//! line cap.

use serde_json::{json, Value};
use std::io::BufRead;

pub(super) fn string_param(params: &Value, key: &str) -> Result<String, String> {
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

pub(super) fn wire_id(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

pub(super) fn success_response(id: Value, result: Value) -> Value {
    json!({"id": id, "result": result})
}

pub(super) fn error_response(id: Value, code: &str, message: &str) -> Value {
    json!({"id": id, "error": {"code": code, "message": message}})
}

pub(super) fn read_frame<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    loop {
        let available = input.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Ok(Some(frame))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map(|index| index + 1).unwrap_or(available.len());
        if frame.len().saturating_add(take) > MAX_FRAME_BYTES {
            input.consume(take);
            if newline.is_none() {
                discard_to_newline(input)?;
            }
            return Err(format!("frame exceeds {} bytes", MAX_FRAME_BYTES));
        }
        frame.extend_from_slice(&available[..take]);
        input.consume(take);
        if newline.is_some() {
            while matches!(frame.last(), Some(b'\n' | b'\r')) {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}

fn discard_to_newline<R: BufRead>(input: &mut R) -> Result<(), String> {
    loop {
        let available = input.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(index) = available.iter().position(|byte| *byte == b'\n') {
            input.consume(index + 1);
            return Ok(());
        }
        let len = available.len();
        input.consume(len);
    }
}
