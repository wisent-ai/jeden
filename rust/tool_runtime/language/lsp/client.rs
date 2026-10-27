//! Talking to one language server over its framed protocol, and keeping it
//! alive between requests.
//!
//! Split out of `tool_runtime/language/lsp.rs`, which had grown past the
//! module line cap.

use super::discovery::{executable_exists, language_id};
use crate::tool_runtime::ToolRuntime;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
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
        // A server that cannot be killed outlives jeden, so that is said, and
        // so is a killed server that cannot be reaped.
        if let Err(error) = self.child.kill() {
            eprintln!(
                "jeden: LSP server {} cannot be killed: {error}",
                self.program
            );
        }
        let process = self.child.id();
        if let Err(error) = stado_wait::blocking(
            stado_wait::Kind::Process,
            format!("LSP server process {process} to end"),
            &self.program,
            || self.child.wait(),
        ) {
            eprintln!(
                "jeden: LSP server {} (process {process}) could not be reaped: {error}",
                self.program
            );
        }
    }
}

/// Reads the server's frames on their own thread and hands each message to
/// the client. The first failure ends the thread and is handed over as its
/// cause; a client that has already gone reads nothing more, so a failed
/// hand-over ends the thread too.
fn reader_thread(stdout: impl Read + Send + 'static, sender: mpsc::Sender<Result<Value, String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let message = read_frame(&mut reader);
            let ended = message.is_err();
            if sender.send(message).is_err() || ended {
                return;
            }
        }
    });
}

/// One framed message: header lines up to a blank one, of which
/// `Content-Length` is required, then that many bytes of JSON. A stream that
/// closes, a length that is not a byte count, a header without one and a
/// body cut short are each refused by name; a frame whose length cannot be
/// read cannot be skipped, since the stream is then out of step.
fn read_frame(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(|error| error.to_string())?;
        if header.is_empty() {
            return Err("LSP server closed stdout".into());
        }
        if header.trim().is_empty() {
            break;
        }
        if let Some(value) = header.trim().strip_prefix("Content-Length:") {
            let length = value.trim().parse::<u64>().map_err(|error| {
                format!("the LSP server's Content-Length {:?} is not a byte count: {error}", value.trim())
            })?;
            content_length = Some(length);
        }
    }
    let length = content_length.ok_or("the LSP server sent a frame header without Content-Length")?;
    let mut bytes = Vec::new();
    reader
        .take(length)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != length {
        return Err(format!(
            "the LSP server closed stdout {} bytes into a {length}-byte message",
            bytes.len()
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
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
        .map_err(|()| format!("cannot convert {} to an LSP root URI", cwd.display()))
}

pub(super) fn file_uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(|uri| uri.to_string())
        .map_err(|()| format!("cannot convert {} to an LSP URI", path.display()))
}

/// Wait for the server's answer to one request. The receive blocks; an
/// announced cancellation reaches it as a null message from the client's
/// cancellation wake, after which this turn's own flag decides. A server
/// indexing a large repository is not a failure.
pub(super) fn await_response(
    runtime: &ToolRuntime<'_>,
    client: &mut LspClient,
    id: &str,
) -> Result<Value, String> {
    loop {
        if runtime.operation.cancellation().is_cancelled() {
            return Err("LSP request cancelled".into());
        }
        match client.messages.recv() {
            Ok(Ok(message)) if message.get("id").and_then(Value::as_str) == Some(id) => {
                if let Some(error) = message.get("error") {
                    return Err(format!("LSP error: {error}"));
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| format!("the LSP server answered request {id} with neither result nor error"));
            }
            Ok(Ok(_notification_or_wake)) => {}
            Ok(Err(error)) => return Err(client.stopped(&error)),
            // Every sender is gone: the reader thread ended after handing over
            // its last message, and the cancellation wake with the client.
            Err(mpsc::RecvError) => return Err(client.stopped("its reader ended")),
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
    let id = uuid::Uuid::new_v4().to_string();
    send(
        &mut client.stdin,
        &json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"processId":std::process::id(),"rootUri":root_uri(runtime.cwd)?,"capabilities":{"textDocument":{"publishDiagnostics":{},"definition":{},"references":{},"rename":{},"codeAction":{},"formatting":{}}}}}),
    )?;
    await_response(runtime, &mut client, &id)?;
    send(
        &mut client.stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )?;
    Ok(client)
}

pub(super) fn ensure_open(client: &mut LspClient, path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    // The document version is its modification time: read as a constant,
    // every later change of the file looked already sent and the server kept
    // answering about the first text it saw.
    let modified = metadata.modified().map_err(|error| {
        format!(
            "{} has no modification time to version: {error}",
            path.display()
        )
    })?;
    let since_epoch = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            format!(
                "{} was modified before the Unix epoch: {error}",
                path.display()
            )
        })?;
    let version = i64::try_from(since_epoch.as_millis()).map_err(|error| {
        format!(
            "{} has a modification time past an i64 version: {error}",
            path.display()
        )
    })?;
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
        let language_id = language_id(path)?;
        send(
            &mut client.stdin,
            &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":language_id,"version":version,"text":text}}}),
        )?;
    }
    client.opened.insert(path.to_path_buf(), version);
    Ok(())
}
