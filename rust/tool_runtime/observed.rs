//! Every public tool invocation keeps its own request and actual outcome.
//! Nested calls inherit a standalone invocation's newly created session.
use super::ToolRuntime;
use crate::agent::observations::{ObservedCall, OperationKind};
use serde_json::{json, Value};

pub fn execute(runtime: &ToolRuntime<'_>, tool: &str, input: &Value) -> Result<Value, String> {
    let mut request = json!({ "arguments": input, "operationId": runtime.operation.operation_id(),
        "parentOperationId": runtime.operation.parent_operation_id(), "turnId": runtime.operation.turn_id() });
    request["cwd"] = serde_json::to_value(runtime.cwd).map_err(|cause| {
        format!("cannot record the workspace for {tool}: {cause}; execution refused")
    })?;
    let observed = ObservedCall::begin(
        runtime.operation.session_id(),
        Some(runtime.cwd),
        OperationKind::Tool,
        tool,
        request,
    )?;
    let result = match runtime.operation.session_id() {
        Some(_) => super::execute_observed(runtime, tool, input),
        None => match observed.session_path().to_str() {
            Some(session) => {
                let scoped = ToolRuntime {
                    cwd: runtime.cwd,
                    artifact_dir: runtime.artifact_dir,
                    operation: runtime.operation.clone().with_identity(
                        runtime.operation.operation_id(),
                        Some(session.to_owned()),
                        runtime.operation.turn_id().map(str::to_owned),
                        runtime.operation.parent_operation_id().map(str::to_owned),
                    ),
                    allow_write: runtime.allow_write,
                    allow_command: runtime.allow_command,
                    interactive: runtime.interactive,
                    ask_user: runtime.ask_user,
                };
                super::execute_observed(&scoped, tool, input)
            }
            None => Err(format!(
                "cannot attach the recorded session {} to {tool}: its path is not UTF-8",
                observed.session_path().display()
            )),
        },
    };
    observed.finish(result, |value| {
        let status = if value.get("ok").and_then(Value::as_bool) == Some(false) {
            "failed"
        } else {
            "completed"
        };
        json!({ "status": status, "output": value })
    })
}
