use super::*;

pub(in crate::agent) fn action_or_text(content: &str) -> Result<Action, String> {
    match extract_json_object(content) {
        Ok(_) => parse_action(content),
        Err(error) if error.starts_with("model returned non-json content") => Ok(Action::Final {
            text: content.to_string(),
        }),
        Err(error) => Err(error),
    }
}

pub(in crate::agent) fn action_to_value(action: &Action) -> Value {
    match action {
        Action::Final { text } => json!({ "action": "final", "text": text }),
        Action::Tool { tool, input } => json!({ "action": "tool", "tool": tool, "input": input }),
        Action::Tools { tools } => {
            json!({ "action": "tools", "tools": tools.iter().map(tool_to_value).collect::<Vec<_>>() })
        }
    }
}

pub(in crate::agent) fn tool_to_value(action: &ToolAction) -> Value {
    json!({ "tool": action.tool, "input": action.input })
}
/// Record a tool call the turn refused to run, with the refusal as its result.
pub(in crate::agent) fn record_unexecuted_tool_action(
    recorder: &mut SessionRecorder,
    step: u32,
    action: &ToolAction,
    result: &Value,
    hooks: &RunHooks<'_>,
) -> Result<(), String> {
    recorder.record(
        "tool_call",
        json!({ "step": step, "tool": action.tool, "input": action.input }),
    )?;
    hooks.trace(&TraceEvent::ToolCall {
        tool: &action.tool,
        input: &action.input,
    });
    recorder.record(
        "tool_result",
        json!({ "step": step, "tool": action.tool, "result": result }),
    )?;
    hooks.trace(&TraceEvent::ToolResult {
        tool: &action.tool,
        result,
    });
    Ok(())
}

pub(in crate::agent) fn run_tool_action(
    args: &Args,
    recorder: &mut SessionRecorder,
    step: u32,
    action: &ToolAction,
    hooks: &RunHooks<'_>,
    allow_write: bool,
    allow_command: bool,
) -> Result<Value, String> {
    recorder.record(
        "tool_call",
        json!({ "step": step, "tool": action.tool, "input": action.input }),
    )?;
    hooks.trace(&TraceEvent::ToolCall {
        tool: &action.tool,
        input: &action.input,
    });
    let artifact_dir = recorder.artifact_dir();
    let runtime = crate::tool_runtime::ToolRuntime {
        cwd: &args.cwd,
        artifact_dir: Some(&artifact_dir),
        operation: hooks.operation_context(&artifact_dir),
        allow_write,
        allow_command,
        interactive: hooks.interactive,
        ask_user: hooks.ask_user.as_deref(),
    };
    let result = match crate::tool_runtime::execute(&runtime, &action.tool, &action.input) {
        Ok(result) => result,
        Err(error) => json!({ "ok": false, "error": error }),
    };
    recorder.record(
        "tool_result",
        json!({ "step": step, "tool": action.tool, "result": result }),
    )?;
    hooks.trace(&TraceEvent::ToolResult {
        tool: &action.tool,
        result: &result,
    });
    Ok(result)
}
