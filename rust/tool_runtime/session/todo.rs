use serde_json::Value;

use crate::tool_runtime::ToolRuntime;

pub(crate) fn todo_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let artifacts = runtime
        .artifact_dir
        .ok_or("todo requires an active session")?;
    let session = crate::completion::session_from_artifacts(artifacts)?;
    crate::completion::execute_todo(&session, input)
}
