//! Reading and writing the newline-delimited messages an external tool server
//! speaks. The server is one the operator configured; a message is as long as
//! the server writes it, and a line that is not JSON ends the connection.
//!
//! Split out of `mcp/client.rs`, which had grown past the module line cap.

use serde_json::Value;
use std::io::Read;
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub(super) fn encode_message(message: &Value) -> Result<Vec<u8>, String> {
    let mut body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
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
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut pending = Vec::new();
        let mut scanned = 0;
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
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
                            break;
                        };
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
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&buffer).into_owned());
    });
    rx
}
