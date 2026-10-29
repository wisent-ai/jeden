use super::config;
use super::types::{
    bounded_json, check_operation, command_exists, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::btree_map::Entry;
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

/// The `seq` of a client's first message, and how far each later one advances.
const FIRST_SEQUENCE: u64 = 1; // https://microsoft.github.io/debug-adapter-protocol/specification#Base_Protocol_ProtocolMessage
const SEQUENCE_STEP: u64 = 1; // https://microsoft.github.io/debug-adapter-protocol/specification#Base_Protocol_ProtocolMessage
/// The session a call that names none uses: every such call shares it.
const SHARED_SESSION: &str = "default";

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

/// The `debug_session` commands that end a session, as DAP spells them.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum SessionEnd {
    Disconnect,
    Terminate,
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
        let process = self.child.id();
        let reaped = stado_wait::blocking(
            stado_wait::Kind::Process,
            format!("debug adapter process {process}"),
            &self.adapter,
            || self.child.wait(),
        );
        if let Err(error) = reaped {
            eprintln!("jeden: debug adapter process {process} could not be reaped: {error}");
        }
    }
}

pub(crate) struct DebuggerService {
    command: Option<String>,
    args: Result<Vec<String>, String>,
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
        if let Err(unread) = &self.args {
            return HealthDescriptor::unavailable("debugger", unread.clone());
        }
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
        let unavailable = || ServiceError::Unavailable {
            service: "debugger",
            detail: self.health().detail,
        };
        let command = self.command.as_deref().ok_or_else(unavailable)?;
        let args = self.args.as_ref().map_err(|_unread| unavailable())?;
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
            .args(args)
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
            sequence: FIRST_SEQUENCE,
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
        let session_key = match input.get("session") {
            None => SHARED_SESSION.to_string(),
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            Some(other) => {
                return Err(ServiceError::InvalidInput(format!(
                    "session must name a session as non-empty text, not {other}"
                )))
            }
        };
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ServiceError::InvalidInput("command is required".into()))?;
        if tool == "debug_session" && crate::vocabulary::named::<SessionEnd>(command).is_ok() {
            let mut sessions = self.sessions.lock();
            let Some(mut session) = sessions.remove(&session_key) else {
                return Err(ServiceError::InvalidInput(format!(
                    "debug session {session_key} does not exist"
                )));
            };
            let request = dap_request(session.sequence, command, input.get("arguments"));
            write_frame(&mut session.stdin, &request)?;
            let sequence = session.sequence;
            let response = wait_response(&mut session, context, sequence)?;
            drop(session);
            return bounded_json(context, "debugger", &response);
        }
        let mut sessions = self.sessions.lock();
        let session = match sessions.entry(session_key) {
            Entry::Occupied(running) => running.into_mut(),
            Entry::Vacant(absent) => absent.insert(self.spawn()?),
        };
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
        session.sequence = session.sequence.saturating_add(SEQUENCE_STEP);
        let request = dap_request(seq, command, input.get("arguments"));
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
            Err(closed) => {
                return Err(session.stopped(&format!("its response channel closed: {closed}")))
            }
        }
    }
}
