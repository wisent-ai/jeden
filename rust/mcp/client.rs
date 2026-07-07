use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use super::{resolve_server_cwd, string_field, MAX_STDERR_BYTES, MCP_PROTOCOL_VERSION};

fn encode_message(message: &Value) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body);
    Ok(out)
}

fn header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_messages(buffer: &mut Vec<u8>) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    loop {
        let Some(end) = header_end(buffer) else { break };
        let header = std::str::from_utf8(&buffer[..end]).map_err(|e| e.to_string())?;
        let Some(line) = header.lines().find(|line| line.to_ascii_lowercase().starts_with("content-length:")) else {
            return Err("MCP response missing Content-Length".into());
        };
        let length = line["Content-Length:".len()..]
            .trim()
            .parse::<usize>()
            .map_err(|_| "invalid MCP Content-Length".to_string())?;
        let body_start = end + 4;
        let body_end = body_start + length;
        if buffer.len() < body_end {
            break;
        }
        let body = buffer[body_start..body_end].to_vec();
        buffer.drain(..body_end);
        messages.push(serde_json::from_slice(&body).map_err(|e| e.to_string())?);
    }
    Ok(messages)
}

fn read_messages(mut stdout: impl Read + Send + 'static) -> Receiver<Result<Value, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    buffer.extend_from_slice(&chunk[..count]);
                    match parse_messages(&mut buffer) {
                        Ok(messages) => {
                            for message in messages {
                                if tx.send(Ok(message)).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error.to_string()));
                    return;
                }
            }
        }
    });
    rx
}

fn drain_stderr(mut stderr: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = String::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk[..count]));
                    if buffer.len() > MAX_STDERR_BYTES {
                        buffer = buffer.chars().rev().take(MAX_STDERR_BYTES).collect::<String>().chars().rev().collect();
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(buffer);
    });
    rx
}

pub(super) struct McpClient {
    child: Child,
    responses: Receiver<Result<Value, String>>,
    stderr: Receiver<String>,
    next_id: u64,
}

impl McpClient {
    pub(super) fn start(server: &Value, cwd: &Path) -> Result<Self, String> {
        if !server.is_object() {
            return Err("server config is required".into());
        }
        if string_field(server, "type").unwrap_or("stdio") != "stdio" {
            return Err("only stdio MCP servers are supported".into());
        }
        let command = string_field(server, "command").ok_or("server.command is required")?;
        let args = server
            .get("args")
            .and_then(Value::as_array)
            .map(|values| values.iter().map(|value| value.as_str().map(ToString::to_string).unwrap_or_else(|| value.to_string())).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut builder = Command::new(command);
        builder
            .args(args)
            .current_dir(resolve_server_cwd(cwd, server))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(env_values) = server.get("env").and_then(Value::as_object) {
            for (key, value) in env_values {
                if value.is_null() {
                    builder.env_remove(key);
                } else {
                    builder.env(key, value.as_str().map(ToString::to_string).unwrap_or_else(|| value.to_string()));
                }
            }
        }
        let mut child = builder.spawn().map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().ok_or("MCP server stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("MCP server stderr unavailable")?;
        Ok(Self { child, responses: read_messages(stdout), stderr: drain_stderr(stderr), next_id: 1 })
    }

    pub(super) fn request(&mut self, method: &str, params: Value, timeout_ms: u64) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let encoded = encode_message(&message)?;
        let stdin = self.child.stdin.as_mut().ok_or("MCP server stdin unavailable")?;
        stdin.write_all(&encoded).map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;

        let wait = Duration::from_millis(timeout_ms.clamp(1_000, 120_000));
        loop {
            let message = match self.responses.recv_timeout(wait) {
                Ok(Ok(message)) => message,
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(format!("MCP request exceeded {timeout_ms}ms wait")),
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                if let Some(text) = error.get("message").and_then(Value::as_str) {
                    return Err(text.to_string());
                }
                return Err(error.to_string());
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let encoded = encode_message(&message)?;
        let stdin = self.child.stdin.as_mut().ok_or("MCP server stdin unavailable")?;
        stdin.write_all(&encoded).map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())
    }

    pub(super) fn initialize(&mut self, timeout_ms: u64) -> Result<(), String> {
        let _ = self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "jeden", "version": "0.1.0"},
            }),
            timeout_ms,
        )?;
        self.notify("notifications/initialized", json!({}))
    }

    fn close(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.stderr.try_recv();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}
