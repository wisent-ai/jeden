use super::config;
use super::types::{
    bounded_json, check_operation, command_exists, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

mod frames;
use frames::{dap_request, read_frame, write_frame};

pub(crate) const TOOLS: &[(&str, &str)] = &[
    (
        "debug_session",
        "Launch, attach, disconnect, or terminate a reusable DAP session",
    ),
    (
        "debug_request",
        "Send a typed request to a reusable DAP session",
    ),
];

struct DapSession {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<ServiceResult<Value>>,
    sequence: u64,
    adapter: String,
    stderr_log: PathBuf,
}
impl DapSession {
    /// Why the adapter stopped answering: the transport `cause`, its exit
    /// status when it has ended, and where what it printed on stderr is kept.
    fn stopped(&mut self, cause: &str) -> ServiceError {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => "still running".to_string(),
            Err(error) => format!("exit status unreadable: {error}"),
        };
        ServiceError::Backend {
            service: "debugger",
            detail: format!(
                "DAP adapter {} stopped answering: {cause} ({status}); its stderr is kept in {}",
                self.adapter,
                self.stderr_log.display()
            ),
        }
    }
}

/// The `debug_session` commands that end a session.
#[derive(Clone, Copy)]
enum SessionEnd {
    Disconnect,
    Terminate,
}

impl SessionEnd {
    fn named(command: &str) -> Option<Self> {
        match command {
            "disconnect" => Some(Self::Disconnect),
            "terminate" => Some(Self::Terminate),
            _ => None,
        }
    }
}
impl Drop for DapSession {
    fn drop(&mut self) {
        // An adapter that cannot be killed outlives jeden, so that is said.
        if let Err(error) = self.child.kill() {
            eprintln!(
                "jeden: debug adapter process {} cannot be killed: {error}",
                self.child.id()
            );
        }
        let _ = self.child.wait();
    }
}

pub(crate) struct DebuggerService {
    command: Option<String>,
    args: Vec<String>,
    cwd: PathBuf,
    sessions: Mutex<BTreeMap<String, DapSession>>,
}
impl DebuggerService {
    pub(crate) fn discover(cwd: &Path, value: &Value) -> Self {
        Self {
            command: config::string(
                value,
                &["toolServices", "debugger", "command"],
                "JEDEN_DAP_ADAPTER",
            ),
            args: config::strings(value, &["toolServices", "debugger", "args"]),
            cwd: cwd.to_path_buf(),
            sessions: Mutex::new(BTreeMap::new()),
        }
    }
    pub(crate) fn health(&self) -> HealthDescriptor {
        match self.command.as_deref() {
            Some(command) if command_exists(command) => {
                HealthDescriptor::healthy("debugger", command)
            }
            Some(command) => HealthDescriptor::unavailable(
                "debugger",
                format!("configured DAP adapter {command} is not executable"),
            ),
            None => HealthDescriptor::unavailable(
                "debugger",
                "set JEDEN_DAP_ADAPTER or toolServices.debugger.command",
            ),
        }
    }
    fn spawn(&self) -> ServiceResult<DapSession> {
        let command = self
            .command
            .as_deref()
            .ok_or_else(|| ServiceError::Unavailable {
                service: "debugger",
                detail: self.health().detail,
            })?;
        let stderr_log = self.cwd.join(".jeden/dap").join(format!(
            "{}.stderr.log",
            Path::new(command)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| ServiceError::Backend {
                    service: "debugger",
                    detail: format!("DAP adapter {command} names no file"),
                })?
        ));
        let log_failed = |error: std::io::Error| ServiceError::Backend {
            service: "debugger",
            detail: format!("cannot create {}: {error}", stderr_log.display()),
        };
        if let Some(directory) = stderr_log.parent() {
            std::fs::create_dir_all(directory).map_err(log_failed)?;
        }
        let stderr = std::fs::File::create(&stderr_log).map_err(log_failed)?;
        let mut child = Command::new(command)
            .args(&self.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|e| ServiceError::Backend {
                service: "debugger",
                detail: format!("cannot start DAP adapter {command}: {e}"),
            })?;
        let stdin = child.stdin.take().ok_or_else(|| ServiceError::Protocol {
            service: "debugger",
            detail: "adapter stdin unavailable".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ServiceError::Protocol {
            service: "debugger",
            detail: "adapter stdout unavailable".into(),
        })?;
        let (tx, rx) = mpsc::channel();
        // One wake per adapter session: an announced cancellation sends a null
        // frame that `wait_response` reads as an event before re-checking its
        // own flag; the wake ends with the session's receiver.
        let wake = tx.clone();
        crate::tool_runtime::runtime_ops::wake_on_cancellation(move || {
            wake.send(Ok(Value::Null)).is_ok()
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let result = read_frame(&mut reader);
                let stop = result.is_err();
                if tx.send(result).is_err() || stop {
                    break;
                }
            }
        });
        Ok(DapSession {
            child,
            stdin,
            responses: rx,
            sequence: 1,
            adapter: command.to_string(),
            stderr_log,
        })
    }
    pub(crate) fn execute(
        &self,
        tool: &str,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        check_operation(context)?;
        let health = self.health();
        if !health.available() {
            return Err(ServiceError::Unavailable {
                service: "debugger",
                detail: health.detail,
            });
        }
        let session_key = input
            .get("session")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string();
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ServiceError::InvalidInput("command is required".into()))?;
        if tool == "debug_session" && SessionEnd::named(command).is_some() {
            let mut sessions = self.sessions.lock();
            let Some(mut session) = sessions.remove(&session_key) else {
                return Err(ServiceError::InvalidInput(format!(
                    "debug session {session_key} does not exist"
                )));
            };
            let request = dap_request(
                session.sequence,
                command,
                input.get("arguments").cloned().unwrap_or_else(|| json!({})),
            );
            write_frame(&mut session.stdin, &request)?;
            let sequence = session.sequence;
            let response = wait_response(&mut session, context, sequence)?;
            drop(session);
            return bounded_json(context, "debugger", &response);
        }
        let mut sessions = self.sessions.lock();
        if !sessions.contains_key(&session_key) {
            sessions.insert(session_key.clone(), self.spawn()?);
        }
        let session = sessions
            .get_mut(&session_key)
            .expect("inserted DAP session");
        if session
            .child
            .try_wait()
            .map_err(|e| ServiceError::Backend {
                service: "debugger",
                detail: e.to_string(),
            })?
            .is_some()
        {
            return Err(session.stopped("the adapter exited before this request"));
        }
        let seq = session.sequence;
        session.sequence = session.sequence.saturating_add(1);
        let request = dap_request(
            seq,
            command,
            input.get("arguments").cloned().unwrap_or_else(|| json!({})),
        );
        write_frame(&mut session.stdin, &request)?;
        let response = wait_response(session, context, seq)?;
        bounded_json(context, "debugger", &response)
    }
}

fn wait_response(
    session: &mut DapSession,
    context: &OperationContext<'_>,
    request_seq: u64,
) -> ServiceResult<Value> {
    // The adapter answers this request or its connection ends; a cancelled
    // turn wakes the receive through the session's cancellation wake.
    loop {
        check_operation(context)?;
        match session.responses.recv() {
            Ok(Ok(value))
                if value.get("type").and_then(Value::as_str) == Some("response")
                    && value.get("request_seq").and_then(Value::as_u64) == Some(request_seq) =>
            {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    return Err(ServiceError::Backend {
                        service: "debugger",
                        detail: match value.get("message").and_then(Value::as_str) {
                            Some(message) => message.to_string(),
                            None => format!("the adapter refused the request: {value}"),
                        },
                    });
                }
                return Ok(value);
            }
            Ok(Ok(_event)) => continue,
            Ok(Err(error)) => return Err(session.stopped(&error.to_string())),
            Err(_) => return Err(session.stopped("its response channel closed")),
        }
    }
}
