//! Talking to one language server over its framed protocol, and keeping it
//! alive between requests.
//!
//! Split out of `tool_runtime/language/lsp.rs`, which had grown past the
//! module line cap.

use super::discovery::{executable_exists, language_id};
use crate::tool_runtime::language::lsp::NEXT_REQUEST;
use crate::tool_runtime::ToolRuntime;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub(super) struct LspClient {
    pub(super) child: Child,
    pub(super) stdin: ChildStdin,
    messages: Receiver<Result<Value, String>>,
    opened: BTreeMap<PathBuf, i64>,
    program: String,
    stderr_log: PathBuf,
}

impl LspClient {
    /// Why the server stopped answering: the transport `cause`, the server's
    /// exit status when it has ended, and where what it printed on stderr is
    /// kept.
    fn stopped(&mut self, cause: &str) -> String {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => "still running".to_string(),
            Err(error) => format!("exit status unreadable: {error}"),
        };
        format!(
            "LSP server {} stopped answering: {cause} ({status}); its stderr is kept in {}",
            self.program,
            self.stderr_log.display()
        )
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // A server that cannot be killed outlives jeden, so that is said.
        if let Err(error) = self.child.kill() {
            eprintln!(
                "jeden: LSP server {} cannot be killed: {error}",
                self.program
            );
        }
        let _ = self.child.wait();
    }
}

fn reader_thread(stdout: impl Read + Send + 'static, sender: mpsc::Sender<Result<Value, String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut content_length = None;
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) => {
                        let _ = sender.send(Err("LSP server closed stdout".into()));
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        return;
                    }
                }
                if header.trim().is_empty() {
                    break;
                }
                if let Some(value) = header.trim().strip_prefix("Content-Length:") {
                    content_length = value.trim().parse::<usize>().ok();
                }
            }
            let Some(length) = content_length else {
                continue;
            };
            let mut bytes = vec![0; length];
            if let Err(error) = reader.read_exact(&mut bytes) {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
            let message = serde_json::from_slice(&bytes).map_err(|error| error.to_string());
            if sender.send(message).is_err() {
                return;
            }
        }
    });
}

pub(super) fn send(stdin: &mut ChildStdin, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    stdin
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .map_err(|error| error.to_string())?;
    stdin.write_all(&body).map_err(|error| error.to_string())?;
    stdin.flush().map_err(|error| error.to_string())
}

pub(super) fn root_uri(cwd: &Path) -> Result<String, String> {
    url::Url::from_directory_path(cwd)
        .map(|uri| uri.to_string())
        .map_err(|_| "cannot convert cwd to LSP root URI".into())
}

pub(super) fn file_uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(|uri| uri.to_string())
        .map_err(|_| "cannot convert path to LSP URI".into())
}

/// Wait for the server's answer to one request. The receive blocks; an
/// announced cancellation reaches it as a null message from the client's
/// cancellation wake, after which this turn's own flag decides. A server
/// indexing a large repository is not a failure.
pub(super) fn await_response(
    runtime: &ToolRuntime<'_>,
    client: &mut LspClient,
    id: u64,
) -> Result<Value, String> {
    loop {
        if runtime.operation.cancellation().is_cancelled() {
            return Err("LSP request cancelled".into());
        }
        match client.messages.recv() {
            Ok(Ok(message)) if message.get("id").and_then(Value::as_u64) == Some(id) => {
                if let Some(error) = message.get("error") {
                    return Err(format!("LSP error: {error}"));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            Ok(Ok(_notification_or_wake)) => {}
            Ok(Err(error)) => return Err(client.stopped(&error)),
            Err(_) => return Err(client.stopped("its reader ended")),
        }
    }
}

pub(super) fn start(
    runtime: &ToolRuntime<'_>,
    program: &str,
    args: &[String],
) -> Result<LspClient, String> {
    if !executable_exists(program) {
        return Err(format!("LSP server executable not found: {program}"));
    }
    let stderr_log = runtime.cwd.join(".jeden/lsp").join(format!(
        "{}.stderr.log",
        Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| format!("LSP server program {program} names no file"))?
    ));
    let log_directory = stderr_log
        .parent()
        .ok_or_else(|| format!("{} has no directory", stderr_log.display()))?;
    fs::create_dir_all(log_directory)
        .map_err(|error| format!("cannot create {}: {error}", log_directory.display()))?;
    let stderr = fs::File::create(&stderr_log)
        .map_err(|error| format!("cannot create {}: {error}", stderr_log.display()))?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(runtime.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| format!("cannot start LSP server {program}: {error}"))?;
    let stdin = child.stdin.take().ok_or("LSP server stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("LSP server stdout unavailable")?;
    let (sender, messages) = mpsc::channel();
    // One wake per client: it ends with the client, whose dropped receiver
    // makes the send fail and removes it.
    let wake = sender.clone();
    crate::tool_runtime::runtime_ops::wake_on_cancellation(move || {
        wake.send(Ok(Value::Null)).is_ok()
    });
    reader_thread(stdout, sender);
    let mut client = LspClient {
        child,
        stdin,
        messages,
        opened: BTreeMap::new(),
        program: program.to_string(),
        stderr_log,
    };
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    send(
        &mut client.stdin,
        &json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"processId":std::process::id(),"rootUri":root_uri(runtime.cwd)?,"capabilities":{"textDocument":{"publishDiagnostics":{},"definition":{},"references":{},"rename":{},"codeAction":{},"formatting":{}}}}}),
    )?;
    let _ = await_response(runtime, &mut client, id)?;
    send(
        &mut client.stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )?;
    Ok(client)
}

pub(super) fn ensure_open(client: &mut LspClient, path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let version = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(1);
    if client.opened.get(path) == Some(&version) {
        return Ok(());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let uri = file_uri(path)?;
    if client.opened.contains_key(path) {
        send(
            &mut client.stdin,
            &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]}}),
        )?;
    } else {
        send(
            &mut client.stdin,
            &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":language_id(path),"version":version,"text":text}}}),
        )?;
    }
    client.opened.insert(path.to_path_buf(), version);
    Ok(())
}
