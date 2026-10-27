//! Transport and nested tools keep observations in the native session ledger,
//! independently of a conversation's action parser or completion policy.
use super::{now_stamp, SessionRecorder};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OperationKind {
    Model,
    Tool,
}

pub(crate) struct ObservedCall<'a> {
    session: PathBuf,
    request: String,
    operation: &'a str,
    kind: OperationKind,
    _held: File,
}

impl<'a> ObservedCall<'a> {
    pub(crate) fn begin(
        parent: Option<&str>,
        cwd: Option<&Path>,
        kind: OperationKind,
        operation: &'a str,
        input: Value,
    ) -> Result<Self, String> {
        let session = match parent {
            Some(parent) => crate::cli::sessions::recorded_session_dir(parent)?,
            None => {
                let current;
                let cwd = match cwd {
                    Some(cwd) => cwd,
                    None => {
                        current = std::env::current_dir().map_err(|cause| {
                            format!("cannot locate the workspace for recording {operation}: {cause}; execution refused")
                        })?;
                        &current
                    }
                };
                let mut recorder = SessionRecorder::new(cwd)?;
                recorder.ensure()?;
                recorder.path()
            }
        };
        let transcript = session.join("transcript.jsonl");
        let held = OpenOptions::new()
            .append(true)
            .open(&transcript)
            .map_err(|cause| {
                format!(
                    "cannot hold {} open while {operation} runs: {cause}; execution refused",
                    transcript.display()
                )
            })?;
        let request = now_stamp().and_then(|stamp| {
            crate::cli::sessions::append_observation(
                &session,
                stamp,
                json!({ "type": "operation_request", "operationKind": kind, "operation": operation, "input": input }),
            )
        })
        .map_err(|cause| {
            format!(
                "cannot record {operation} before execution in {}: {cause}",
                session.display()
            )
        })?;
        Ok(Self {
            session,
            request,
            operation,
            kind,
            _held: held,
        })
    }

    pub(crate) fn session_path(&self) -> &Path {
        &self.session
    }

    /// A failed result is kept as a failure, never replaced with an empty answer.
    /// If persistence itself fails, return both the operation's actual error
    /// and the write failure instead of claiming an observed result was kept.
    pub(crate) fn finish<T>(
        self,
        result: Result<T, String>,
        output: impl FnOnce(&T) -> Value,
    ) -> Result<T, String> {
        let outcome = match &result {
            Ok(value) => output(value),
            Err(cause) => json!({ "status": "failed", "cause": cause }),
        };
        self.record_result(outcome)?;
        result
    }

    pub(crate) fn record_result(&self, outcome: Value) -> Result<(), String> {
        now_stamp().and_then(|stamp| {
            crate::cli::sessions::append_observation(
                &self.session,
                stamp,
                json!({ "type": "operation_result", "operationKind": self.kind, "operation": self.operation, "requestEvent": self.request, "outcome": outcome }),
            )
        })
        .map(|_appended_line| ())
        .map_err(|cause| format!("cannot record the result of {} (request {}) in {}: {cause}; observed outcome: {outcome}", self.operation, self.request, self.session.display()))
    }
}
