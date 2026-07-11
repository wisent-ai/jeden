use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

const ABSENCE_TIMEOUT: Duration = Duration::from_millis(300);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

enum OutputEvent {
    Message(Value),
    Invalid(String),
    Eof,
}

enum ChildCommand {
    Kill,
}

struct AcpProcess {
    stdin: Option<ChildStdin>,
    output: Receiver<OutputEvent>,
    child_command: Sender<ChildCommand>,
    exit: Receiver<std::io::Result<ExitStatus>>,
    finished: bool,
}

impl AcpProcess {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jeden"))
            .arg("acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn jeden acp");
        let stdin = child.stdin.take().expect("piped ACP stdin");
        let stdout = child.stdout.take().expect("piped ACP stdout");

        let (output_tx, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => match serde_json::from_str(&line) {
                        Ok(message) => {
                            if output_tx.send(OutputEvent::Message(message)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = output_tx.send(OutputEvent::Invalid(format!(
                                "invalid ACP JSON {line:?}: {error}"
                            )));
                            return;
                        }
                    },
                    Err(error) => {
                        let _ = output_tx.send(OutputEvent::Invalid(format!(
                            "failed reading ACP stdout: {error}"
                        )));
                        return;
                    }
                }
            }
            let _ = output_tx.send(OutputEvent::Eof);
        });

        let (child_command, commands) = mpsc::channel();
        let (exit_tx, exit) = mpsc::channel();
        thread::spawn(move || loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = exit_tx.send(Ok(status));
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = exit_tx.send(Err(error));
                    return;
                }
            }
            match commands.recv_timeout(Duration::from_millis(20)) {
                Ok(ChildCommand::Kill) => {
                    let result = child.kill().and_then(|_| child.wait());
                    let _ = exit_tx.send(result);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        });

        Self {
            stdin: Some(stdin),
            output,
            child_command,
            exit,
            finished: false,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("ACP stdin remains open");
        serde_json::to_writer(&mut *stdin, &message).expect("serialize ACP request");
        stdin.write_all(b"\n").expect("frame ACP request");
        stdin.flush().expect("flush ACP request");
    }

    fn response(&self) -> Value {
        match self.output.recv_timeout(RESPONSE_TIMEOUT) {
            Ok(OutputEvent::Message(message)) => message,
            Ok(OutputEvent::Invalid(error)) => panic!("{error}"),
            Ok(OutputEvent::Eof) => panic!("ACP process closed stdout before responding"),
            Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for ACP response"),
            Err(RecvTimeoutError::Disconnected) => panic!("ACP stdout reader disconnected"),
        }
    }

    fn assert_no_output(&self) {
        match self.output.recv_timeout(ABSENCE_TIMEOUT) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(OutputEvent::Message(message)) => panic!("unexpected ACP output: {message}"),
            Ok(OutputEvent::Invalid(error)) => panic!("{error}"),
            Ok(OutputEvent::Eof) => panic!("ACP process exited while output absence was expected"),
            Err(RecvTimeoutError::Disconnected) => panic!("ACP stdout reader disconnected"),
        }
    }

    fn finish_on_eof(mut self) {
        self.stdin.take();
        match self.output.recv_timeout(RESPONSE_TIMEOUT) {
            Ok(OutputEvent::Eof) => {}
            Ok(OutputEvent::Message(message)) => {
                panic!("unexpected ACP output during EOF teardown: {message}")
            }
            Ok(OutputEvent::Invalid(error)) => panic!("{error}"),
            Err(RecvTimeoutError::Timeout) => panic!("ACP stdout remained open after stdin EOF"),
            Err(RecvTimeoutError::Disconnected) => panic!("ACP stdout reader disconnected"),
        }
        let status = self
            .exit
            .recv_timeout(RESPONSE_TIMEOUT)
            .expect("ACP process exits after stdin EOF")
            .expect("wait for ACP process");
        assert!(
            status.success(),
            "ACP process failed during EOF teardown: {status}"
        );
        self.finished = true;
    }
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        if !self.finished {
            self.stdin.take();
            let _ = self.child_command.send(ChildCommand::Kill);
            let _ = self.exit.recv_timeout(RESPONSE_TIMEOUT);
        }
    }
}

struct Workspace(PathBuf);

impl Workspace {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jeden-acp-{label}-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create ACP test workspace");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn initialize(process: &mut AcpProcess, id: u64) -> Value {
    process.send(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "acp-wire-test", "version": "1"}
        }
    }));
    process.response()
}

fn new_session(process: &mut AcpProcess, cwd: &Path, id: u64) -> String {
    process.send(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {"cwd": cwd, "mcpServers": []}
    }));
    let response = process.response();
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], id);
    response["result"]["sessionId"]
        .as_str()
        .expect("session/new returns a typed sessionId")
        .to_owned()
}

#[test]
fn startup_is_silent_and_initialize_negotiates_pinned_v1_capabilities() {
    let mut process = AcpProcess::spawn();

    process.assert_no_output();
    let response = initialize(&mut process, 1);

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        response["result"]["agentCapabilities"]["promptCapabilities"],
        json!({"image": false, "audio": false, "embeddedContext": false})
    );
    assert!(response["result"].get("capabilities").is_none());
}

#[test]
fn typed_session_vectors_reject_unsupported_content_and_keep_cancel_one_way() {
    let workspace = Workspace::new("typed-session");
    let mut process = AcpProcess::spawn();
    let initialize_response = initialize(&mut process, 10);
    assert_eq!(initialize_response["result"]["protocolVersion"], 1);

    let session_id = new_session(&mut process, workspace.path(), 11);
    let session_path = Path::new(&session_id);
    assert!(
        session_path.is_absolute(),
        "sessionId names the absolute durable session path"
    );
    assert!(
        session_path.exists(),
        "sessionId resolves to the created session resource"
    );

    process.send(json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "image", "data": "AA==", "mimeType": "image/png"}]
        }
    }));
    let prompt_error = process.response();
    assert_eq!(prompt_error["jsonrpc"], "2.0");
    assert_eq!(prompt_error["id"], 12);
    assert_eq!(prompt_error["error"]["code"], -32602);
    assert_eq!(prompt_error["error"]["message"], "Invalid params");
    assert_eq!(
        prompt_error["error"]["data"],
        "image prompt content is not supported"
    );

    process.send(json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {"sessionId": session_id}
    }));
    process.assert_no_output();
}

#[test]
fn legacy_capability_and_session_aliases_are_method_not_found() {
    let mut process = AcpProcess::spawn();
    let initialize_response = initialize(&mut process, 20);
    assert_eq!(initialize_response["result"]["protocolVersion"], 1);

    for (id, method) in [(21, "capabilities"), (22, "new")] {
        process.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {}
        }));
        let response = process.response();
        assert_eq!(response["jsonrpc"], "2.0", "{method}");
        assert_eq!(response["id"], id, "{method}");
        assert_eq!(response["error"]["code"], -32601, "{method}");
        assert_eq!(response["error"]["message"], "Method not found", "{method}");
        assert!(response.get("result").is_none(), "{method}");
    }
}

#[test]
fn loading_an_unknown_session_returns_typed_resource_not_found() {
    let workspace = Workspace::new("missing-session");
    let mut process = AcpProcess::spawn();
    let initialize_response = initialize(&mut process, 30);
    assert_eq!(initialize_response["result"]["protocolVersion"], 1);

    process.send(json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "session/load",
        "params": {
            "sessionId": "missing-acp-session",
            "cwd": workspace.path(),
            "mcpServers": []
        }
    }));
    let response = process.response();
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 31);
    assert_eq!(response["error"]["code"], -32002);
    assert_eq!(response["error"]["message"], "Resource not found");
    assert_eq!(
        response["error"]["data"],
        json!({"uri": "missing-acp-session"})
    );
}

#[test]
fn stdin_eof_terminates_the_adapter_successfully() {
    AcpProcess::spawn().finish_on_eof();
}
