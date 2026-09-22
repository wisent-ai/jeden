//! Reading and writing the newline-delimited messages an external tool server
//! speaks, without letting a misbehaving server exhaust this process.
//!
//! Split out of `mcp/client.rs`, which had grown past the module line cap.

use super::super::MAX_STDERR_BYTES;
use serde_json::Value;
use std::io::Read;
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub(crate) const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUED_MESSAGES: usize = 64;

pub(super) fn encode_message(message: &Value) -> Result<Vec<u8>, String> {
    let mut body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err("MCP message exceeds 8 MiB limit".into());
    }
    body.push(b'\n');
    Ok(body)
}

pub(super) fn parse_json_line(line: &[u8]) -> Result<Option<Value>, String> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(line)
        .map(Some)
        .map_err(|error| format!("invalid newline-delimited MCP JSON: {error}"))
}

pub(super) fn read_messages(
    mut stdout: impl Read + Send + 'static,
) -> Receiver<Result<Value, String>> {
    let (tx, rx) = mpsc::sync_channel(MAX_QUEUED_MESSAGES);
    thread::spawn(move || {
        let mut pending = Vec::new();
        let mut scanned = 0;
        let mut chunk = [0u8; 8192];
        loop {
            let read_limit = chunk.len().min(MAX_MESSAGE_BYTES + 1 - pending.len());
            match stdout.read(&mut chunk[..read_limit]) {
                Ok(0) => {
                    if !pending.iter().all(u8::is_ascii_whitespace) {
                        let _ = tx.send(Err(
                            "MCP stdio closed with an unterminated JSON message".into()
                        ));
                    }
                    return;
                }
                Ok(count) => {
                    pending.extend_from_slice(&chunk[..count]);
                    loop {
                        let Some(end) = pending[scanned..]
                            .iter()
                            .position(|byte| *byte == b'\n')
                            .map(|offset| scanned + offset)
                        else {
                            scanned = pending.len();
                            if pending.len() > MAX_MESSAGE_BYTES {
                                let _ = tx.send(Err("MCP message exceeds 8 MiB limit".into()));
                                return;
                            }
                            break;
                        };
                        if end > MAX_MESSAGE_BYTES {
                            let _ = tx.send(Err("MCP message exceeds 8 MiB limit".into()));
                            return;
                        }
                        let parsed = parse_json_line(&pending[..=end]);
                        pending.drain(..=end);
                        scanned = 0;
                        match parsed {
                            Ok(Some(message)) => {
                                if tx.send(Ok(message)).is_err() {
                                    return;
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let _ = tx.send(Err(error));
                                return;
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(format!("MCP stdio read failed: {error}")));
                    return;
                }
            }
        }
    });
    rx
}

pub(super) fn drain_stderr(mut stderr: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    buffer.extend_from_slice(&chunk[..count]);
                    if buffer.len() > MAX_STDERR_BYTES {
                        let excess = buffer.len() - MAX_STDERR_BYTES;
                        buffer.drain(..excess);
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&buffer).into_owned());
    });
    rx
}
