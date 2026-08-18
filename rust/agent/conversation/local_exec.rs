use super::*;

/// Transcript blocks for `!`/`$` escapes cap each output stream at this many
/// lines; overflow is summarized with a truncation marker.
const LOCAL_OUTPUT_MAX_LINES: usize = 200;

impl Conversation {
    /// Run a `! <shell>` / `$ <python>` escape from the interactive prompt.
    /// The call goes through the same approval policy and tool runtime a
    /// model-initiated run_command/python_eval tool call would use, and the
    /// outcome is recorded in the session ledger. Nothing enters the model
    /// conversation: no model turn happens and the ledger event kinds are
    /// skipped by session replay.
    pub(crate) fn local_tool_exec(
        &mut self,
        args: &Args,
        hooks: &RunHooks<'_>,
        tool: &str,
        code: &str,
    ) -> Result<String, String> {
        let input = match tool {
            "python_eval" => json!({ "code": code }),
            _ => json!({ "command": code }),
        };
        if let Some(reason) = crate::hooks::pretool_block(
            &args.cwd,
            tool,
            &input,
            args.allow_command,
            &self.recorder.path().join("transcript.jsonl"),
        ) {
            return Ok(format!("Blocked by PreToolUse hook: {reason}"));
        }
        let result = match resolve_tool_approval(args, tool, &input, hooks) {
            ToolDecision::Allow {
                allow_write,
                allow_command,
            } => {
                // `interaction` entries are part of the durable session ledger
                // but are skipped by conversation replay, so the escape stays
                // invisible to the model even after a resume.
                self.recorder.record(
                    "interaction",
                    json!({ "type": "local_tool_call", "tool": tool, "input": input }),
                )?;
                let artifact_dir = self.recorder.artifact_dir();
                let runtime = crate::tool_runtime::ToolRuntime {
                    cwd: &args.cwd,
                    artifact_dir: Some(&artifact_dir),
                    operation: hooks.operation_context(&artifact_dir),
                    allow_write,
                    allow_command,
                    interactive: hooks.interactive,
                    ask_user: hooks.ask_user.as_deref(),
                };
                let result = match crate::tool_runtime::execute(&runtime, tool, &input) {
                    Ok(result) => result,
                    Err(error) => json!({ "ok": false, "error": error }),
                };
                self.recorder.record(
                    "interaction",
                    json!({ "type": "local_tool_result", "tool": tool, "result": result }),
                )?;
                crate::hooks::posttool(&args.cwd, tool, &result, args.allow_command);
                result
            }
            ToolDecision::Deny(reason) => return Ok(reason),
        };
        Ok(format_local_result(&result))
    }
}

fn push_stream_lines(lines: &mut Vec<String>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let stream = text.trim_end_matches(['\r', '\n']);
    let total = stream.split('\n').count();
    for line in stream.split('\n').take(LOCAL_OUTPUT_MAX_LINES) {
        lines.push(line.trim_end_matches('\r').to_string());
    }
    if total > LOCAL_OUTPUT_MAX_LINES {
        lines.push(format!(
            "… {} more line(s) truncated",
            total - LOCAL_OUTPUT_MAX_LINES
        ));
    }
}

fn format_local_result(result: &Value) -> String {
    let mut lines = Vec::new();
    push_stream_lines(
        &mut lines,
        result.get("stdout").and_then(Value::as_str).unwrap_or(""),
    );
    let stderr = result.get("stderr").and_then(Value::as_str).unwrap_or("");
    if !stderr.trim().is_empty() {
        lines.push("stderr:".to_string());
        push_stream_lines(&mut lines, stderr);
    }
    let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !ok {
        if let Some(code) = result.get("code").and_then(Value::as_i64) {
            lines.push(format!("(exit code: {code})"));
        } else if result.get("timedOut").and_then(Value::as_bool) == Some(true) {
            lines.push("(timed out)".to_string());
        } else if result.get("cancelled").and_then(Value::as_bool) == Some(true) {
            lines.push("(cancelled)".to_string());
        }
        if let Some(error) = result
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.trim().is_empty())
        {
            lines.push(format!("error: {}", error.trim()));
        }
    }
    if lines.is_empty() {
        lines.push("(no output)".to_string());
    }
    lines.join("\n")
}
