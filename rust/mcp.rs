use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_STDERR_BYTES: usize = 100_000;

fn dirs_home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn read_json_value(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

pub fn load_config(cwd: &Path) -> Value {
    let user = read_json_value(&dirs_home().join(".jeden/mcp.json"));
    let project = read_json_value(&cwd.join(".jeden/mcp.json"));
    let mut servers = serde_json::Map::new();
    for source in [&user, &project] {
        if let Some(map) = source.get("mcpServers").and_then(Value::as_object) {
            for (name, server) in map {
                servers.insert(name.clone(), server.clone());
            }
        }
    }
    let mut disabled = Vec::new();
    for source in [&user, &project] {
        if let Some(values) = source.get("disabledServers").and_then(Value::as_array) {
            disabled.extend(values.iter().filter_map(Value::as_str).map(ToString::to_string));
        }
    }
    json!({"mcpServers": servers, "disabledServers": disabled})
}

pub fn configured_server_names(cwd: &Path) -> Vec<String> {
    let config = load_config(cwd);
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut names = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .keys()
                .map(|name| if disabled.contains(name.as_str()) { format!("{} (disabled)", name) } else { name.clone() })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn configured_server(cwd: &Path, server_name: &str) -> Result<Value, String> {
    let config = load_config(cwd);
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    if disabled.contains(server_name) {
        return Err(format!("disabled MCP server: {server_name}"));
    }
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(server_name))
        .cloned()
        .ok_or_else(|| format!("unknown MCP server: {server_name}"))
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn resolve_server_cwd(cwd: &Path, server: &Value) -> PathBuf {
    match string_field(server, "cwd") {
        Some(raw) if Path::new(raw).is_absolute() => PathBuf::from(raw),
        Some(raw) => cwd.join(raw),
        None => cwd.to_path_buf(),
    }
}

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

struct McpClient {
    child: Child,
    responses: Receiver<Result<Value, String>>,
    stderr: Receiver<String>,
    next_id: u64,
}

impl McpClient {
    fn start(server: &Value, cwd: &Path) -> Result<Self, String> {
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

    fn request(&mut self, method: &str, params: Value, timeout_ms: u64) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let encoded = encode_message(&message)?;
        let stdin = self.child.stdin.as_mut().ok_or("MCP server stdin unavailable")?;
        stdin.write_all(&encoded).map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;

        let timeout = Duration::from_millis(timeout_ms.clamp(1_000, 120_000));
        loop {
            let message = match self.responses.recv_timeout(timeout) {
                Ok(Ok(message)) => message,
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(format!("MCP request timed out after {timeout_ms}ms")),
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

    fn initialize(&mut self, timeout_ms: u64) -> Result<(), String> {
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

fn with_client(cwd: &Path, server_name: &str, timeout_ms: u64, callback: impl FnOnce(&mut McpClient) -> Result<Value, String>) -> Result<Value, String> {
    let server = configured_server(cwd, server_name)?;
    let mut client = McpClient::start(&server, cwd)?;
    client.initialize(timeout_ms)?;
    callback(&mut client)
}

pub fn list_tools(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_client(cwd, server_name, timeout_ms, |client| client.request("tools/list", json!({}), timeout_ms))
}

pub fn call_tool(cwd: &Path, server_name: &str, tool_name: &str, args: Value, timeout_ms: u64) -> Result<Value, String> {
    if tool_name.is_empty() {
        return Err("toolName is required".into());
    }
    with_client(cwd, server_name, timeout_ms, |client| client.request("tools/call", json!({"name": tool_name, "arguments": args}), timeout_ms))
}

pub fn list_resources(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_client(cwd, server_name, timeout_ms, |client| client.request("resources/list", json!({}), timeout_ms))
}

pub fn read_resource(cwd: &Path, server_name: &str, uri: &str, timeout_ms: u64) -> Result<Value, String> {
    if uri.is_empty() {
        return Err("uri is required".into());
    }
    with_client(cwd, server_name, timeout_ms, |client| client.request("resources/read", json!({"uri": uri}), timeout_ms))
}

pub fn list_prompts(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_client(cwd, server_name, timeout_ms, |client| client.request("prompts/list", json!({}), timeout_ms))
}

pub fn get_prompt(cwd: &Path, server_name: &str, name: &str, args: Value, timeout_ms: u64) -> Result<Value, String> {
    if name.is_empty() {
        return Err("name is required".into());
    }
    with_client(cwd, server_name, timeout_ms, |client| client.request("prompts/get", json!({"name": name, "arguments": args}), timeout_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir() -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        env::temp_dir().join(format!("jeden-mcp-smoke-{}-{stamp}", std::process::id()))
    }

    #[test]
    fn smoke_lists_and_calls_stdio_tool() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join(".jeden")).unwrap();
        let server = root.join("mcp-server.mjs");
        fs::write(&server, r#"
let buffer = Buffer.alloc(0)
let calls = 0
function send(message) {
  const body = Buffer.from(JSON.stringify(message), 'utf8')
  process.stdout.write('Content-Length: ' + body.length + '\r\n\r\n')
  process.stdout.write(body)
}
function handle(message) {
  if (message.method === 'initialize') {
    send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: {}, serverInfo: { name: 'fake', version: '1' } } })
    return
  }
  if (message.method === 'tools/list') {
    send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'echo', description: 'Echo text through MCP', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] } })
    return
  }
  if (message.method === 'tools/call') {
    calls += 1
    send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: message.params.arguments.text + ':' + calls }] } })
  }
}
process.stdin.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk])
  for (;;) {
    const headerEnd = buffer.indexOf('\r\n\r\n')
    if (headerEnd === -1) break
    const header = buffer.subarray(0, headerEnd).toString('utf8')
    const length = Number(header.split('\r\n')[0].slice('Content-Length:'.length).trim())
    const bodyStart = headerEnd + 4
    const bodyEnd = bodyStart + length
    if (buffer.length < bodyEnd) break
    const message = JSON.parse(buffer.subarray(bodyStart, bodyEnd).toString('utf8'))
    buffer = buffer.subarray(bodyEnd)
    handle(message)
  }
})
"#).unwrap();
        fs::write(
            root.join(".jeden/mcp.json"),
            serde_json::to_string(&json!({"mcpServers": {"local": {"command": "node", "args": [server.to_string_lossy()]}}})).unwrap(),
        ).unwrap();

        let listed = list_tools(&root, "local", 2_000).unwrap();
        assert_eq!(listed["tools"][0]["name"], "echo");
        let called = call_tool(&root, "local", "echo", json!({"text": "rust"}), 2_000).unwrap();
        assert_eq!(called["content"][0]["text"], "rust:1");
        let _ = fs::remove_dir_all(root);
    }
}
