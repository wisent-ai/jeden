//! Sending one message to an external tool server and collecting what comes
//! back, over either kind of connection.
//!
//! Split out of `mcp/client.rs`, which had grown past the module line cap.

use super::framing::encode_message;
use super::{McpClient, Transport, MCP_SESSION_ID};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::Value;
use std::io::{Read, Write};
use std::sync::mpsc;

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
                        .map_err(|error| format!("invalid MCP session id header: {error}"))?;
                    transport.session_id = Some(value.to_string());
                }
                // A notification the server accepted carries no messages.
                if response.status() == reqwest::StatusCode::ACCEPTED {
                    return Ok(Vec::new());
                }
                // An answer that declares no content type is read as JSON; one
                // whose declaration is not text is refused naming the cause.
                let content_type = match response.headers().get(CONTENT_TYPE) {
                    None => None,
                    Some(value) => Some(
                        value
                            .to_str()
                            .map_err(|error| format!("invalid MCP content type header: {error}"))?
                            .to_string(),
                    ),
                };
                // The server the operator configured answers as much as the
                // call produced; it is read to its end.
                let mut body = Vec::new();
                let mut response = response;
                response
                    .read_to_end(&mut body)
                    .map_err(|error| format!("MCP HTTP read failed: {error}"))?;
                if body.is_empty() {
                    return Ok(Vec::new());
                }
                match content_type.as_deref() {
                    Some(declared) if declared.starts_with("text/event-stream") => {
                        let text = std::str::from_utf8(&body).map_err(|error| {
                            format!("invalid MCP event stream UTF-8: {error}")
                        })?;
                        let mut messages = Vec::new();
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data:") {
                                messages.push(serde_json::from_str(data.trim()).map_err(
                                    |error| format!("invalid MCP event data: {error}"),
                                )?);
                            }
                        }
                        Ok(messages)
                    }
                    Some(declared) if !declared.starts_with("application/json") => {
                        Err(format!("unsupported MCP HTTP content type: {declared}"))
                    }
                    Some(_) | None => Ok(vec![serde_json::from_slice(&body).map_err(|error| {
                        format!("invalid MCP HTTP JSON: {error}")
                    })?]),
                }
            }
        }
    }
}
