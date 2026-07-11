use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::tool_runtime::shared::{jail_path, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

const MAX_LSP_MESSAGE: usize = 8 * 1024 * 1024;
const MAX_LSP_SERVERS: usize = 8;
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
static SERVERS: LazyLock<Mutex<BTreeMap<String, LspClient>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

struct LspClient {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Value, String>>,
    opened: BTreeMap<PathBuf, i64>,
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn executable_exists(name: &str) -> bool {
    if name.contains(std::path::MAIN_SEPARATOR) { return Path::new(name).is_file(); }
    env::var_os("PATH").map(|paths| env::split_paths(&paths).any(|path| path.join(name).is_file())).unwrap_or(false)
}
fn probe_server(name: &str) -> bool {
    if !executable_exists(name) { return false; }
    let Ok(mut child)=Command::new(name).arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else{return false;};
    let deadline=Instant::now()+Duration::from_secs(2);
    loop { match child.try_wait() { Ok(Some(status))=>return status.success(),Ok(None) if Instant::now()<deadline=>thread::sleep(Duration::from_millis(10)),_=>{let _=child.kill();let _=child.wait();return false;} } }
}

pub(super) fn healthy_servers() -> Vec<String> {
    ["rust-analyzer","pyright-langserver","typescript-language-server"].into_iter().filter(|name|probe_server(name)).map(ToOwned::to_owned).collect()
}


fn inferred_server(path: &Path) -> Option<(String, Vec<String>)> {
    match path.extension().and_then(|value| value.to_str()).unwrap_or("") {
        "rs" if executable_exists("rust-analyzer") => Some(("rust-analyzer".into(), Vec::new())),
        "py" if executable_exists("pyright-langserver") => Some(("pyright-langserver".into(), vec!["--stdio".into()])),
        "js" | "jsx" | "ts" | "tsx" if executable_exists("typescript-language-server") => Some(("typescript-language-server".into(), vec!["--stdio".into()])),
        _ => None,
    }
}

fn command_for(input: &Value, path: &Path) -> Result<(String, Vec<String>), String> {
    if let Some(program) = string_input(input, "server") {
        let args = input.get("serverArgs").and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect()).unwrap_or_default();
        return Ok((program, args));
    }
    inferred_server(path).ok_or_else(|| format!("no healthy LSP server discovered for {}", path.display()))
}

fn reader_thread(stdout: impl Read + Send + 'static, sender: mpsc::Sender<Result<Value, String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut content_length = None;
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) => { let _ = sender.send(Err("LSP server closed stdout".into())); return; }
                    Ok(_) => {}
                    Err(error) => { let _ = sender.send(Err(error.to_string())); return; }
                }
                if header == "\r\n" || header == "\n" { break; }
                if let Some(value) = header.trim().strip_prefix("Content-Length:") {
                    content_length = value.trim().parse::<usize>().ok();
                }
            }
            let Some(length) = content_length else { continue; };
            if length > MAX_LSP_MESSAGE { let _ = sender.send(Err("LSP message exceeds 8 MiB".into())); return; }
            let mut bytes = vec![0; length];
            if let Err(error) = reader.read_exact(&mut bytes) { let _ = sender.send(Err(error.to_string())); return; }
            let message = serde_json::from_slice(&bytes).map_err(|error| error.to_string());
            if sender.send(message).is_err() { return; }
        }
    });
}

fn send(stdin: &mut ChildStdin, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    stdin.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).map_err(|error| error.to_string())?;
    stdin.write_all(&body).map_err(|error| error.to_string())?;
    stdin.flush().map_err(|error| error.to_string())
}

fn root_uri(cwd: &Path) -> Result<String, String> {
    url::Url::from_directory_path(cwd).map(|uri| uri.to_string()).map_err(|_| "cannot convert cwd to LSP root URI".into())
}

fn file_uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path).map(|uri| uri.to_string()).map_err(|_| "cannot convert path to LSP URI".into())
}

fn await_response(runtime: &ToolRuntime<'_>, client: &mut LspClient, id: u64, timeout: Duration) -> Result<Value, String> {
    let deadline = runtime.operation.effective_deadline(timeout);
    loop {
        if runtime.operation.cancellation().is_cancelled() { return Err("LSP request cancelled".into()); }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err("LSP request timed out".into()); }
        match client.messages.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(Ok(message)) if message.get("id").and_then(Value::as_u64) == Some(id) => {
                if let Some(error) = message.get("error") { return Err(format!("LSP error: {error}")); }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            Ok(Ok(_notification)) => {}
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("LSP reader stopped".into()),
        }
    }
}

fn start(runtime: &ToolRuntime<'_>, program: &str, args: &[String]) -> Result<LspClient, String> {
    if !executable_exists(program) { return Err(format!("LSP server executable not found: {program}")); }
    let mut child = Command::new(program).args(args).current_dir(runtime.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|error| error.to_string())?;
    let stdin = child.stdin.take().ok_or("LSP server stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("LSP server stdout unavailable")?;
    let (sender, messages) = mpsc::channel();
    reader_thread(stdout, sender);
    let mut client = LspClient { child, stdin, messages, opened: BTreeMap::new() };
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    send(&mut client.stdin, &json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"processId":std::process::id(),"rootUri":root_uri(runtime.cwd)?,"capabilities":{"textDocument":{"publishDiagnostics":{},"definition":{},"references":{},"rename":{},"codeAction":{},"formatting":{}}}}}))?;
    let _ = await_response(runtime, &mut client, id, Duration::from_secs(20))?;
    send(&mut client.stdin, &json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
    Ok(client)
}

fn language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()).unwrap_or("") {
        "rs" => "rust", "py" => "python", "ts" => "typescript", "tsx" => "typescriptreact", "jsx" => "javascriptreact", _ => "javascript",
    }
}

fn ensure_open(client: &mut LspClient, path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let version = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|duration| duration.as_millis() as i64).unwrap_or(1);
    if client.opened.get(path) == Some(&version) { return Ok(()); }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let uri = file_uri(path)?;
    if client.opened.contains_key(path) {
        send(&mut client.stdin, &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]}}))?;
    } else {
        send(&mut client.stdin, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":language_id(path),"version":version,"text":text}}}))?;
    }
    client.opened.insert(path.to_path_buf(), version);
    Ok(())
}

pub(crate) fn lsp(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let action = string_input(input, "action").unwrap_or_else(|| "health".into());
    if action == "health" {
        let healthy=healthy_servers();
        let servers = ["rust-analyzer", "pyright-langserver", "typescript-language-server"].into_iter().map(|name| json!({"name":name,"healthy":healthy.iter().any(|item|item==name)})).collect::<Vec<_>>();
        return Ok(json!({"ok":true,"status":if healthy.is_empty(){"unavailable"}else{"healthy"},"servers":servers,"maxServers":MAX_LSP_SERVERS}));
    }
    let label = string_input(input, "path").ok_or("LSP action requires path")?;
    let path = jail_path(runtime.cwd, &label)?;
    let (program, args) = command_for(input, &path)?;
    let key = format!("{}\0{}\0{}", runtime.cwd.display(), program, args.join("\0"));
    let mut servers = SERVERS.lock().map_err(|_| "LSP manager poisoned")?;
    if !servers.contains_key(&key) {
        if servers.len() >= MAX_LSP_SERVERS { return Err(format!("LSP server limit reached ({MAX_LSP_SERVERS})")); }
        servers.insert(key.clone(), start(runtime, &program, &args)?);
    }
    let client = servers.get_mut(&key).ok_or("LSP server unavailable")?;
    if let Some(status) = client.child.try_wait().map_err(|error| error.to_string())? { return Err(format!("LSP server exited: {status}")); }
    ensure_open(client, &path)?;
    let uri = file_uri(&path)?;
    let position = json!({"line":u64_input(input,"line",1).saturating_sub(1),"character":u64_input(input,"column",1).saturating_sub(1)});
    let (method, params) = match action.as_str() {
        "diagnostics" => ("textDocument/diagnostic", json!({"textDocument":{"uri":uri}})),
        "definition" => ("textDocument/definition", json!({"textDocument":{"uri":uri},"position":position})),
        "references" => ("textDocument/references", json!({"textDocument":{"uri":uri},"position":position,"context":{"includeDeclaration":true}})),
        "rename" => ("textDocument/rename", json!({"textDocument":{"uri":uri},"position":position,"newName":string_input(input,"newName").ok_or("LSP rename requires newName")?})),
        "codeActions" => ("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":{"start":position,"end":position},"context":{"diagnostics":input.get("diagnostics").cloned().unwrap_or_else(||json!([]))}})),
        "format" => ("textDocument/formatting", json!({"textDocument":{"uri":uri},"options":{"tabSize":u64_input(input,"tabSize",4),"insertSpaces":input.get("insertSpaces").and_then(Value::as_bool).unwrap_or(true)}})),
        other => return Err(format!("unsupported LSP action: {other}")),
    };
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    send(&mut client.stdin, &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
    let result = await_response(runtime, client, id, Duration::from_millis(u64_input(input,"timeoutMs",20_000).clamp(100,120_000)))?;
    Ok(json!({"ok":true,"action":action,"path":label,"server":program,"result":result}))
}
