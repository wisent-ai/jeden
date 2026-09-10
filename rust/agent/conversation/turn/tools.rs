//! Running one tool the model asked for: the hook gate, the approval
//! decision, the call itself, and the post-tool hook. A single tool action and
//! a batch of them take the same path, so a rule can never hold for one shape
//! of request and not the other.

use super::super::*;

impl Conversation {
    pub(super) fn handle_tool_action(
        &mut self,
        args: &Args,
        step: u32,
        action: &ToolAction,
        tracks_completion: bool,
        hooks: &mut RunHooks,
    ) -> Result<Value, String> {
        if hooks.cancelled() {
            let err = "Turn cancelled.".to_string();
            self.recorder
                .record("run_error", json!({ "message": err }))?;
            return Err(if tracks_completion {
                self.completion_failure("turn_cancelled", &err, hooks)
            } else {
                err
            });
        }
        if let Some(reason) = crate::hooks::pretool_block(
            &args.cwd,
            &action.tool,
            &action.input,
            args.allow_command,
            &self.recorder.path().join("transcript.jsonl"),
        ) {
            hooks.note(&format!("tool blocked by hook: {}", action.tool));
            let result = json!({
                "ok": false,
                "error": format!("blocked by PreToolUse hook: {}", reason),
            });
            record_unexecuted_tool_action(&mut self.recorder, step, action, &result, hooks)?;
            return Ok(result);
        }
        match resolve_tool_approval(args, &action.tool, &action.input, hooks) {
            ToolDecision::Allow {
                allow_write,
                allow_command,
            } => {
                hooks.note(&format!("tool: {}", action.tool));
                let result = run_tool_action(
                    args,
                    &mut self.recorder,
                    step,
                    action,
                    hooks,
                    allow_write,
                    allow_command,
                )?;
                crate::hooks::posttool(&args.cwd, &action.tool, &result, args.allow_command);
                Ok(result)
            }
            ToolDecision::Deny(reason) => {
                hooks.note(&format!("tool denied: {}", action.tool));
                let result = json!({ "ok": false, "error": reason });
                record_unexecuted_tool_action(&mut self.recorder, step, action, &result, hooks)?;
                Ok(result)
            }
        }
    }
}
