use reqwest::blocking::Client as HttpClient;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use super::{resolve_server_cwd, string_field, MAX_STDERR_BYTES, MCP_PROTOCOL_VERSION};

const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUED_MESSAGES: usize = 64;
const MAX_NOTIFICATIONS: usize = 256;
const MAX_SESSION_ID_BYTES: usize = 1024;
const MCP_SESSION_ID: &str = "mcp-session-id";

fn encode_message(message: &Value) -> Result<Vec<u8>, String> {
    let mut body = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err("MCP message exceeds 8 MiB limit".into());
    }
    body.push(b'\n');
    Ok(body)
}

fn parse_json_line(line: &[u8]) -> Result<Option<Value>, String> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(line)
        .map(Some)
        .map_err(|error| format!("invalid newline-delimited MCP JSON: {error}"))
}

fn read_messages(mut stdout: impl Read + Send + 'static) -> Receiver<Result<Value, String>> {
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

fn drain_stderr(mut stderr: impl Read + Send + 'static) -> Receiver<String> {
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

struct StdioTransport {
    child: Child,
    responses: Receiver<Result<Value, String>>,
    stderr: Receiver<String>,
}

struct HttpTransport {
    client: HttpClient,
    url: String,
    session_id: Option<String>,
}

enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

pub(super) struct McpClient {
    transport: Transport,
    next_id: u64,
    notifications: VecDeque<Value>,
}

impl McpClient {
    pub(super) fn start(server: &Value, cwd: &Path) -> Result<Self, String> {
        let sandbox = crate::tool_runtime::runtime_ops::SecureRuntime::detect()
            .health()
            .clone();
        if !sandbox.enforced() {
            return Err(format!(
                "enforced sandbox unavailable: {}: {}",
                sandbox.backend, sandbox.detail
            ));
        }
        let object = server
            .as_object()
            .ok_or("MCP server config must be an object")?;
        let transport_name = string_field(server, "type").unwrap_or_else(|| {
            if object.contains_key("url") {
                "http"
            } else {
                "stdio"
            }
        });
        let transport = match transport_name {
            "stdio" => Transport::Stdio(Self::start_stdio(server, cwd)?),
            "http" | "streamable-http" => Transport::Http(Self::start_http(server)?),
            other => {
                return Err(format!(
                    "unsupported MCP transport '{other}'; expected stdio or streamable-http"
                ))
            }
        };
        Ok(Self {
            transport,
            next_id: 1,
            notifications: VecDeque::new(),
        })
    }

    fn start_stdio(server: &Value, cwd: &Path) -> Result<StdioTransport, String> {
        if server.get("url").is_some() {
            return Err("stdio MCP config must not contain url".into());
        }
        let command = string_field(server, "command")
            .filter(|command| !command.is_empty())
            .ok_or("stdio MCP server.command must be a non-empty string")?;
        let args = match server.get("args") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| "stdio MCP server.args entries must be strings".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("stdio MCP server.args must be an array".into()),
        };
        let mut builder = Command::new(command);
        builder.env_clear();
        builder
            .args(args)
            .current_dir(resolve_server_cwd(cwd, server))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match server.get("env") {
            None => {}
            Some(Value::Object(values)) => {
                for (key, value) in values {
                    match value {
                        Value::Null => {
                            builder.env_remove(key);
                        }
                        Value::String(value) => {
                            builder.env(key, value);
                        }
                        _ => {
                            return Err("stdio MCP server.env values must be strings or null".into())
                        }
                    }
                }
            }
            Some(_) => return Err("stdio MCP server.env must be an object".into()),
        }
        let mut child = builder
            .spawn()
            .map_err(|error| format!("failed to start MCP server: {error}"))?;
        let stdout = child.stdout.take().ok_or("MCP server stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("MCP server stderr unavailable")?;
        Ok(StdioTransport {
            child,
            responses: read_messages(stdout),
            stderr: drain_stderr(stderr),
        })
    }

    fn start_http(server: &Value) -> Result<HttpTransport, String> {
        if server.get("command").is_some() || server.get("args").is_some() {
            return Err("streamable HTTP MCP config must not contain command or args".into());
        }
        let url = string_field(server, "url")
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
            .ok_or("streamable HTTP MCP server.url must be an http(s) URL")?
            .to_string();
        let client = HttpClient::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("failed to create MCP HTTP client: {error}"))?;
        Ok(HttpTransport {
            client,
            url,
            session_id: None,
        })
    }
    fn send(&mut self, message: &Value, timeout_ms: u64) -> Result<Vec<Value>, String> {
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
                let wait = Duration::from_millis(timeout_ms.clamp(1_000, 120_000));
                let deadline = std::time::Instant::now() + wait;
                let mut messages = Vec::new();
                loop {
                    let remaining = deadline
                        .checked_duration_since(std::time::Instant::now())
                        .ok_or_else(|| format!("MCP request exceeded {timeout_ms}ms wait"))?;
                    let response = match transport.responses.recv_timeout(remaining) {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) => return Err(error),
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            return Err(format!("MCP request exceeded {timeout_ms}ms wait"))
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err("MCP stdio transport closed".into())
                        }
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
                    .timeout(Duration::from_millis(timeout_ms.clamp(1_000, 120_000)))
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

    pub(super) fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("MCP request id exhausted")?;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        loop {
            let messages = self.send(&message, timeout_ms)?;
            for response in messages {
                if response.get("method").is_some()
                    && response.get("id").is_none()
                    && self.notifications.len() >= MAX_NOTIFICATIONS
                {
                    return Err("MCP notification queue limit exceeded".into());
                }
                if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return Err("MCP response has invalid jsonrpc version".into());
                }
                if response.get("method").is_some() && response.get("id").is_none() {
                    self.notifications.push_back(response);
                    continue;
                }
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = response.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| error.to_string()));
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or("MCP response is missing result".into());
            }
            if matches!(self.transport, Transport::Http(_)) {
                return Err("MCP HTTP response did not contain the matching request id".into());
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value, timeout_ms: u64) -> Result<(), String> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        for response in self.send(&message, timeout_ms)? {
            if response.get("method").is_some() && response.get("id").is_none() {
                if self.notifications.len() >= MAX_NOTIFICATIONS {
                    return Err("MCP notification queue limit exceeded".into());
                }
                self.notifications.push_back(response);
            }
        }
        Ok(())
    }

    pub(super) fn initialize(&mut self, timeout_ms: u64) -> Result<Value, String> {
        let init = self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "jeden", "version": "0.1.0"},
            }),
            timeout_ms,
        )?;
        if !init.is_object()
            || init
                .get("protocolVersion")
                .and_then(Value::as_str)
                .is_none()
            || !init
                .get("capabilities")
                .map(Value::is_object)
                .unwrap_or(false)
        {
            return Err("MCP initialize result has invalid schema".into());
        }
        self.notify("notifications/initialized", json!({}), timeout_ms)?;
        Ok(init)
    }

    pub(super) fn take_notifications(&mut self) -> Vec<Value> {
        self.notifications.drain(..).collect()
    }

    pub(super) fn poll_notifications(&mut self) -> Result<Vec<Value>, String> {
        if let Transport::Stdio(transport) = &mut self.transport {
            loop {
                match transport.responses.try_recv() {
                    Ok(Ok(message))
                        if message.get("method").is_some() && message.get("id").is_none() =>
                    {
                        self.notifications.push_back(message)
                    }
                    Ok(Ok(_)) => {
                        return Err(
                            "MCP server sent an unexpected response without an active request"
                                .into(),
                        )
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("MCP stdio transport closed".into())
                    }
                }
            }
        }
        Ok(self.notifications.drain(..).collect())
    }

    pub(super) fn is_alive(&mut self) -> bool {
        match &mut self.transport {
            Transport::Stdio(transport) => matches!(transport.child.try_wait(), Ok(None)),
            Transport::Http(_) => true,
        }
    }

    pub(super) fn close(&mut self) {
        match &mut self.transport {
            Transport::Stdio(transport) => {
                drop(transport.child.stdin.take());
                if matches!(transport.child.try_wait(), Ok(None)) {
                    let _ = transport.child.kill();
                }
                let _ = transport.child.wait();
                let _ = transport.stderr.try_recv();
            }
            Transport::Http(transport) => {
                if let Some(session_id) = transport.session_id.take() {
                    let _ = transport
                        .client
                        .delete(&transport.url)
                        .header(MCP_SESSION_ID, session_id)
                        .timeout(Duration::from_secs(2))
                        .send();
                }
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};

    static NEXT_CANARY: AtomicU64 = AtomicU64::new(1);

    struct GatedLineReader {
        next_line: usize,
        line_count: usize,
        reads: mpsc::Sender<usize>,
        release_blocked_line: mpsc::Receiver<()>,
    }

    impl Read for GatedLineReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.next_line == self.line_count {
                return Ok(0);
            }
            self.next_line += 1;
            self.reads.send(self.next_line).unwrap();
            if self.next_line == MAX_QUEUED_MESSAGES + 1 {
                self.release_blocked_line.recv().unwrap();
            }
            let line = b"{}\n";
            buffer[..line.len()].copy_from_slice(line);
            Ok(line.len())
        }
    }

    #[test]
    fn malformed_newline_json_surfaces_a_protocol_error() {
        let responses = read_messages(Cursor::new(b"{\"jsonrpc\":\"2.0\",]\n"));

        let error = responses
            .recv_timeout(Duration::from_secs(1))
            .expect("malformed frame must complete boundedly")
            .expect_err("malformed frame must be rejected");

        assert!(
            error.starts_with("invalid newline-delimited MCP JSON:"),
            "unexpected malformed-frame error: {error}"
        );
    }

    #[test]
    fn frame_over_eight_mib_is_rejected_before_parsing() {
        let mut oversized = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        oversized.push(b'\n');
        let responses = read_messages(Cursor::new(oversized));

        let error = responses
            .recv_timeout(Duration::from_secs(1))
            .expect("oversized frame must complete boundedly")
            .expect_err("oversized frame must be rejected");

        assert_eq!(error, "MCP message exceeds 8 MiB limit");
    }

    #[test]
    fn reader_applies_backpressure_at_the_queue_limit_and_resumes() {
        let (reads_tx, reads_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let responses = read_messages(GatedLineReader {
            next_line: 0,
            line_count: MAX_QUEUED_MESSAGES + 2,
            reads: reads_tx,
            release_blocked_line: release_rx,
        });

        for expected in 1..=MAX_QUEUED_MESSAGES + 1 {
            assert_eq!(
                reads_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                expected
            );
        }
        release_tx.send(()).unwrap();
        assert_eq!(
            reads_rx.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Timeout),
            "reader consumed beyond a full bounded queue"
        );

        assert_eq!(
            responses
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap(),
            json!({})
        );
        assert_eq!(
            reads_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            MAX_QUEUED_MESSAGES + 2,
            "reader did not resume after the consumer freed capacity"
        );
    }

    #[test]
    fn degraded_sandbox_rejects_stdio_before_canary_command_runs() {
        let sandbox = crate::tool_runtime::runtime_ops::SecureRuntime::detect();
        if sandbox.health().enforced() {
            return;
        }
        let canary = std::env::temp_dir().join(format!(
            "jeden-mcp-start-canary-{}-{}",
            std::process::id(),
            NEXT_CANARY.fetch_add(1, Ordering::Relaxed)
        ));
        let server = json!({
            "type": "stdio",
            "command": "/usr/bin/touch",
            "args": [canary.to_string_lossy()]
        });

        let error = McpClient::start(&server, Path::new("/"))
            .err()
            .expect("degraded sandbox must reject stdio startup");

        assert!(
            error.starts_with("enforced sandbox unavailable:"),
            "unexpected sandbox rejection: {error}"
        );
        assert!(
            !canary.exists(),
            "rejected stdio command created its canary"
        );
    }
}
