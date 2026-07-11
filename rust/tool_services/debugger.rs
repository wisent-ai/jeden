use super::config;
use super::types::{
    bounded_json, check_operation, command_exists, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const MAX_DAP_FRAME: usize = 8 * 1024 * 1024;
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
}
impl Drop for DapSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
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
        let mut child = Command::new(command)
            .args(&self.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ServiceError::Backend {
                service: "debugger",
                detail: e.to_string(),
            })?;
        let stdin = child.stdin.take().ok_or_else(|| ServiceError::Protocol {
            service: "debugger",
            detail: "adapter stdin unavailable".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ServiceError::Protocol {
            service: "debugger",
            detail: "adapter stdout unavailable".into(),
        })?;
        let (tx, rx) = mpsc::sync_channel(64);
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
        if tool == "debug_session" && matches!(command, "disconnect" | "terminate") {
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
            let response = wait_response(&session.responses, context, session.sequence)?;
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
            return Err(ServiceError::Backend {
                service: "debugger",
                detail: "adapter exited".into(),
            });
        }
        let seq = session.sequence;
        session.sequence = session.sequence.saturating_add(1);
        let request = dap_request(
            seq,
            command,
            input.get("arguments").cloned().unwrap_or_else(|| json!({})),
        );
        write_frame(&mut session.stdin, &request)?;
        let response = wait_response(&session.responses, context, seq)?;
        bounded_json(context, "debugger", &response)
    }
    #[cfg(test)]
    pub(crate) fn session_count(&self) -> usize {
        self.sessions.lock().len()
    }
}

fn dap_request(seq: u64, command: &str, arguments: Value) -> Value {
    json!({"seq":seq,"type":"request","command":command,"arguments":arguments})
}
fn write_frame(stdin: &mut ChildStdin, value: &Value) -> ServiceResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| ServiceError::Protocol {
        service: "debugger",
        detail: e.to_string(),
    })?;
    write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len())?;
    stdin.write_all(&bytes)?;
    stdin.flush()?;
    Ok(())
}
fn read_frame(reader: &mut BufReader<impl Read>) -> ServiceResult<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(ServiceError::Protocol {
                service: "debugger",
                detail: "adapter closed transport".into(),
            });
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(raw) = line.strip_prefix("Content-Length:") {
            length = raw.trim().parse::<usize>().ok();
        }
    }
    let length = length
        .filter(|v| *v <= MAX_DAP_FRAME)
        .ok_or_else(|| ServiceError::Protocol {
            service: "debugger",
            detail: "missing or excessive Content-Length".into(),
        })?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|e| ServiceError::Protocol {
        service: "debugger",
        detail: e.to_string(),
    })
}
fn wait_response(
    rx: &Receiver<ServiceResult<Value>>,
    context: &OperationContext<'_>,
    request_seq: u64,
) -> ServiceResult<Value> {
    let deadline = context
        .deadline()
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
    loop {
        check_operation(context)?;
        if Instant::now() >= deadline {
            return Err(ServiceError::DeadlineExceeded);
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok(value))
                if value.get("type").and_then(Value::as_str) == Some("response")
                    && value.get("request_seq").and_then(Value::as_u64) == Some(request_seq) =>
            {
                if value.get("success").and_then(Value::as_bool) == Some(false) {
                    return Err(ServiceError::Backend {
                        service: "debugger",
                        detail: value
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("request failed")
                            .into(),
                    });
                }
                return Ok(value);
            }
            Ok(Ok(_event)) => continue,
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ServiceError::Protocol {
                    service: "debugger",
                    detail: "adapter response channel closed".into(),
                })
            }
        }
    }
}
