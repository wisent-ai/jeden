use super::types::{check_operation, ServiceError, ServiceResult};
use crate::tool_runtime::runtime_ops::{
    ManagedCommand, OperationContext, ProcessManager, TerminationReason,
};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

pub(crate) fn run(
    service: &'static str,
    context: &OperationContext<'_>,
    cwd: &Path,
    program: &str,
    args: &[String],
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> ServiceResult<String> {
    check_operation(context)?;
    let mut command = ManagedCommand::new(program, cwd);
    command.args = args.iter().map(OsString::from).collect();
    command.stdin = stdin;
    let result = ProcessManager
        .run(context, command, timeout)
        .map_err(|detail| ServiceError::Backend { service, detail })?;
    match result.reason {
        TerminationReason::Cancelled => return Err(ServiceError::Cancelled),
        TerminationReason::TimedOut => return Err(ServiceError::DeadlineExceeded),
        TerminationReason::Completed => {}
    }
    if !result.status.success() {
        let detail = if result.stderr.text.trim().is_empty() {
            result.stdout.text
        } else {
            result.stderr.text
        };
        return Err(ServiceError::Backend {
            service,
            detail: detail.trim().to_string(),
        });
    }
    Ok(result.stdout.text)
}

pub(crate) fn run_json(
    service: &'static str,
    context: &OperationContext<'_>,
    cwd: &Path,
    program: &str,
    args: &[String],
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> ServiceResult<Value> {
    let output = run(service, context, cwd, program, args, stdin, timeout)?;
    serde_json::from_str(&output).map_err(|error| ServiceError::Protocol {
        service,
        detail: format!("invalid JSON response: {error}"),
    })
}
