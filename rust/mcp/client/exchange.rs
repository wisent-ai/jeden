//! Sending one message to an external tool server and collecting what comes
//! back, over either kind of connection.
//!
//! Split out of `mcp/client.rs`, which had grown past the module line cap.

use super::framing::{encode_message, parse_json_line};
use super::{McpClient, Transport, MAX_NOTIFICATIONS, MAX_SESSION_ID_BYTES, MCP_SESSION_ID};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::Value;
use std::io::Write;

impl McpClient {
    pub(super) fn send(&mut self, message: &Value) -> Result<Vec<Value>, String> {
        match &mut self.transport {
            Transport::Stdio(transport) => {
                let encoded = encode_message(message)?;
                let stdin = transport
                    .child
                    .stdin
                    .as_mut()
                    .ok_or("MCP server stdin unavailable")?;
                stdin
                    .write_all(&encoded)
                    .map_err(|error| format!("MCP stdio write failed: {error}"))?;
                stdin
                    .flush()
                    .map_err(|error| format!("MCP stdio flush failed: {error}"))?;
                let Some(expected_id) = message.get("id").and_then(Value::as_u64) else {
                    return Ok(Vec::new());
                };
                // The server answers, or its transport closes. Nothing here
                // guesses how long a tool call on the other side may take:
                // a `list` that reads a repository and a `call` that runs a
                // build are the same wait to this loop.
                let mut messages = Vec::new();
                loop {
                    let response = match transport.responses.recv() {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) => return Err(error),
                        Err(mpsc::RecvError) => return Err("MCP stdio transport closed".into()),
                    };
                    let complete = response.get("id").and_then(Value::as_u64) == Some(expected_id);
                    messages.push(response);
                    if complete {
                        return Ok(messages);
                    }
                }
            }
            Transport::Http(transport) => {
                let mut request = transport
                    .client
                    .post(&transport.url)
                    .header(CONTENT_TYPE, "application/json")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .json(message);
                if let Some(session_id) = &transport.session_id {
                    request = request.header(MCP_SESSION_ID, session_id);
                }
                let response = request
                    .send()
                    .map_err(|error| format!("MCP HTTP transport failed: {error}"))?;
                if !response.status().is_success() {
                    return Err(format!("MCP HTTP transport returned {}", response.status()));
                }
                if let Some(value) = response.headers().get(MCP_SESSION_ID) {
                    let value = value
                        .to_str()
                        .map_err(|_| "invalid MCP session id header")?;
                    if value.len() > MAX_SESSION_ID_BYTES {
                        return Err("MCP session id exceeds 1024 byte limit".into());
                    }
                    transport.session_id = Some(value.to_string());
                }
                if response.status().as_u16() == 202 || response.content_length() == Some(0) {
                    return Ok(Vec::new());
                }
                let content_type = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let mut body = Vec::new();
                response
                    .take((MAX_MESSAGE_BYTES + 1) as u64)
                    .read_to_end(&mut body)
                    .map_err(|error| format!("MCP HTTP read failed: {error}"))?;
                if body.len() > MAX_MESSAGE_BYTES {
                    return Err("MCP HTTP response exceeds 8 MiB limit".into());
                }
                if content_type.starts_with("text/event-stream") {
                    let text = std::str::from_utf8(&body)
                        .map_err(|error| format!("invalid MCP event stream UTF-8: {error}"))?;
                    let mut messages = Vec::new();
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data:") {
                            if messages.len() >= MAX_NOTIFICATIONS {
                                return Err("MCP event stream exceeds event limit".into());
                            }
                            messages.push(
                                serde_json::from_str(data.trim())
                                    .map_err(|error| format!("invalid MCP event data: {error}"))?,
                            );
                        }
                    }
                    Ok(messages)
                } else if content_type.starts_with("application/json") || content_type.is_empty() {
                    Ok(vec![serde_json::from_slice(&body).map_err(|error| {
                        format!("invalid MCP HTTP JSON: {error}")
                    })?])
                } else {
                    Err(format!("unsupported MCP HTTP content type: {content_type}"))
                }
            }
        }
    }

}
