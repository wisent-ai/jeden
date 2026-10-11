//! What a fired hook decided: a refusal of the tool or answer, or the
//! context it adds to the prompt, read from its exit status and output.

use serde_json::Value;

use super::HookOutcome;

/// Parse a hook's stdout as a JSON object, if it is one.
pub(crate) fn parse_hook_json(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .filter(|v| v.is_object())
}

/// A refusing hook decision (`PreToolUse` for a tool, `Stop` for an answer):
/// `Some(reason)` refuses. A hook refuses by exiting with code 2, by printing
/// JSON `{"decision":"block", "reason":"…"}`, or by failing to execute (the
/// reserved negative infrastructure outcome). Other positive exits remain
/// nonblocking. The reason is the JSON `reason`, else stderr, else stdout,
/// else `fallback`.
pub fn block_decision(outcomes: &[HookOutcome], fallback: &str) -> Option<String> {
    outcomes.iter().find_map(|o| {
        let json = parse_hook_json(&o.stdout);
        let json_block = json
            .as_ref()
            .and_then(|j| j.get("decision"))
            .and_then(Value::as_str)
            .map(|d| d.eq_ignore_ascii_case("block"))
            .unwrap_or(false);
        if !o.exit_code.is_negative() && o.exit_code != 2 && !json_block {
            return None;
        }
        let json_reason = json
            .as_ref()
            .and_then(|j| j.get("reason").or_else(|| j.get("userMessage")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let reason = json_reason
            .map(str::to_string)
            .or_else(|| {
                Some(o.stderr.trim())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                Some(o.stdout.trim())
                    .filter(|s| !s.is_empty() && parse_hook_json(&o.stdout).is_none())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| fallback.to_string());
        Some(reason)
    })
}

/// Collect both native and event-specific hook context without discarding either.
/// An invalid context field is a protocol error, not an empty instruction.
pub fn prompt_context(outcomes: &[HookOutcome]) -> Result<String, String> {
    let mut context = Vec::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        // POSIX successful command status:
        // https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html#tag_19_08_02
        if outcome.exit_code != 0 {
            return Err(format!(
                "context hook at index {index} exited {}: stderr: {}; stdout: {}",
                outcome.exit_code,
                outcome.stderr.trim(),
                outcome.stdout.trim()
            ));
        }
        let Some(json) = parse_hook_json(&outcome.stdout) else {
            let text = outcome.stdout.trim();
            if !text.is_empty() {
                context.push(text.to_owned());
            }
            continue;
        };
        if json.get("decision").and_then(Value::as_str) == Some("block") {
            return Err(format!("context hook at index {index} refused: {json}"));
        }
        let mut append = |value: &Value, field: &str| -> Result<(), String> {
            let text = value.as_str().ok_or_else(|| {
                format!("context hook at index {index}: {field} must be a string; got {value}")
            })?;
            if !text.trim().is_empty() {
                context.push(text.to_owned());
            }
            Ok(())
        };
        if let Some(value) = json.get("additionalContext") {
            append(value, "additionalContext")?;
        }
        if let Some(output) = json.get("hookSpecificOutput") {
            let output = output.as_object().ok_or_else(|| {
                format!("context hook at index {index}: hookSpecificOutput must be an object; got {output}")
            })?;
            if let Some(value) = output.get("additionalContext") {
                append(value, "hookSpecificOutput.additionalContext")?;
            }
        }
    }
    Ok(context.join("\n"))
}
