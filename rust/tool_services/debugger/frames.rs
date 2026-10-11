//! The Debug Adapter Protocol's framing: one JSON message behind a
//! `Content-Length` header, written to the adapter's stdin and read from its
//! stdout.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::ChildStdin;

use serde_json::{json, Value};

use super::super::types::{ServiceError, ServiceResult};

pub(super) fn dap_request(seq: u64, command: &str, arguments: Value) -> Value {
    json!({"seq":seq,"type":"request","command":command,"arguments":arguments})
}

pub(super) fn write_frame(stdin: &mut ChildStdin, value: &Value) -> ServiceResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| ServiceError::Protocol {
        service: "debugger",
        detail: e.to_string(),
    })?;
    write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len())?;
    stdin.write_all(&bytes)?;
    stdin.flush()?;
    Ok(())
}

fn protocol(detail: String) -> ServiceError {
    ServiceError::Protocol {
        service: "debugger",
        detail,
    }
}

/// One message from the adapter. A header that ends the transport, a
/// `Content-Length` that is not a byte count, a header with none, and a body
/// shorter than its header announced are each refused with what was read.
pub(super) fn read_frame(reader: &mut BufReader<impl Read>) -> ServiceResult<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.is_empty() {
            return Err(protocol("adapter closed transport".into()));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(raw) = line.strip_prefix("Content-Length:") {
            let raw = raw.trim();
            length = Some(raw.parse::<usize>().map_err(|error| {
                protocol(format!(
                    "the adapter's frame header names Content-Length {raw:?}, not a byte count: {error}"
                ))
            })?);
        }
    }
    let length = length
        .ok_or_else(|| protocol("the adapter's frame header carried no Content-Length".into()))?;
    let wanted = u64::try_from(length).map_err(|error| {
        protocol(format!(
            "the adapter's frame of {length} bytes cannot be read: {error}"
        ))
    })?;
    let mut bytes = Vec::with_capacity(length);
    reader.by_ref().take(wanted).read_to_end(&mut bytes)?;
    if bytes.len() != length {
        return Err(protocol(format!(
            "the adapter closed its transport after {} of the {length} bytes its frame header announced",
            bytes.len()
        )));
    }
    serde_json::from_slice(&bytes).map_err(|e| protocol(e.to_string()))
}
