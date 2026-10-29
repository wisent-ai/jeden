//! Reading and writing the newline-delimited messages an external tool server
//! speaks. The server is one the operator configured; a message is as long as
//! the server writes it, and a line that is not JSON ends the connection.
//!
//! Split out of `mcp/client.rs`, which had grown past the module line cap.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub(super) fn encode_message(message: &Value) -> Result<Vec<u8>, String> {
    let mut body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    body.push(b'\n');
    Ok(body)
}

/// One line as a message; a line of whitespace alone carries none.
pub(super) fn parse_json_line(line: &[u8]) -> Result<Option<Value>, String> {
    let line = line.trim_ascii();
    if line.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(line)
        .map(Some)
        .map_err(|error| format!("invalid newline-delimited MCP JSON: {error}"))
}

pub(super) fn read_messages(stdout: impl Read + Send + 'static) -> Receiver<Result<Value, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            if let Err(error) = reader.read_until(b'\n', &mut line) {
                // The reader's last word; a client that stopped listening needs none.
                let _unreceived = tx.send(Err(format!("MCP stdio read failed: {error}")));
                return;
            }
            // Nothing read is the server closing its output.
            if line.is_empty() {
                return;
            }
            if line.last() != Some(&b'\n') {
                if !line.trim_ascii().is_empty() {
                    let _unreceived = tx.send(Err(
                        "MCP stdio closed with an unterminated JSON message".into(),
                    ));
                }
                return;
            }
            match parse_json_line(&line) {
                Ok(Some(message)) => {
                    if tx.send(Ok(message)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _unreceived = tx.send(Err(error));
                    return;
                }
            }
        }
    });
    rx
}

/// Everything the server writes to stderr, and why reading it stopped when
/// it stopped before the server closed it.
pub(super) fn drain_stderr(mut stderr: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let ended = stderr.read_to_end(&mut buffer);
        let mut text = String::from_utf8_lossy(&buffer).into_owned();
        if let Err(error) = ended {
            text.push_str(&format!(
                "\n[the server's stderr could not be read further: {error}]"
            ));
        }
        let _unreceived = tx.send(text);
    });
    rx
}
