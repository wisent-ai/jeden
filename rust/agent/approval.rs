use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolTier {
    Read,
    Write,
    Exec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApprovalMode {
    AlwaysAsk,
    Write,
    Yolo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolPolicy {
    Allow,
    Deny,
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolDecision {
    Allow {
        allow_write: bool,
        allow_command: bool,
    },
    Deny(String),
}

pub(super) fn is_builtin_read_tool(tool: &str) -> bool {
    matches!(
        tool,
        "list_dir"
            | "read_file"
            | "read_binary_file"
            | "read_document"
            | "read_archive"
            | "read_image"
            | "read_sqlite"
            | "search_text"
            | "search_files"
            | "glob_paths"
            | "grep_regex"
            | "list_package_scripts"
            | "git_status"
            | "git_diff"
            | "git_log"
            | "git_show"
            | "fetch_url"
            | "fetch_readable_url"
            | "list_artifacts"
            | "read_artifact"
            | "ask_user"
    )
}

pub(super) fn tool_tier(tool: &str) -> ToolTier {
    if is_write_tool(tool)
        || matches!(tool, "save_artifact" | "todo" | "memory")
        || tool.starts_with("mcp_")
        || tool.starts_with("mcp__")
    {
        ToolTier::Write
    } else if is_command_tool(tool) {
        ToolTier::Exec
    } else if is_builtin_read_tool(tool) {
        ToolTier::Read
    } else {
        // Tools without an approval declaration use the exec tier as a safe
        // default for custom or future tools the model may emit.
        ToolTier::Exec
    }
}

pub(super) fn parse_approval_mode(value: &str) -> Option<ApprovalMode> {
    match value.trim() {
        "always-ask" => Some(ApprovalMode::AlwaysAsk),
        "write" => Some(ApprovalMode::Write),
        "yolo" => Some(ApprovalMode::Yolo),
        _ => None,
    }
}

pub(super) fn approval_mode(args: &Args, state: &Value) -> ApprovalMode {
    if args.yolo {
        return ApprovalMode::Yolo;
    }
    if let Some(mode) = state
        .pointer("/tools/approvalMode")
        .and_then(Value::as_str)
        .and_then(parse_approval_mode)
    {
        return mode;
    }
    if args.allow_write && args.allow_command {
        return ApprovalMode::Yolo;
    }
    // Intentional safe default: older Jeden sessions only auto-ran write or
    // command tools with explicit flags. Missing config keeps that gate.
    ApprovalMode::AlwaysAsk
}

pub(super) fn parse_tool_policy(value: &str) -> Option<ToolPolicy> {
    match value.trim() {
        "allow" => Some(ToolPolicy::Allow),
        "deny" => Some(ToolPolicy::Deny),
        "prompt" => Some(ToolPolicy::Prompt),
        _ => None,
    }
}

pub(super) fn tool_policy(state: &Value, tool: &str) -> Option<ToolPolicy> {
    state
        .pointer(&format!(
            "/tools/approval/{}",
            tool.replace('~', "~0").replace('/', "~1")
        ))
        .and_then(Value::as_str)
        .and_then(parse_tool_policy)
}

pub(super) fn safety_override_reason(tool: &str, input: &Value) -> Option<String> {
    let command = match tool {
        "run_command" => input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "run_process" => {
            let mut parts = Vec::new();
            if let Some(cmd) = input.get("command").and_then(Value::as_str) {
                parts.push(cmd.to_string());
            }
            if let Some(args) = input.get("args").and_then(Value::as_array) {
                parts.extend(args.iter().filter_map(Value::as_str).map(str::to_string));
            }
            parts.join(" ")
        }
        _ => return None,
    };
    let lower = command.to_ascii_lowercase();
    let compact = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.contains("rm -rf /") || compact.contains("rm -rf /*") || compact.contains("rm -rf ~")
    {
        return Some("Critical destructive delete pattern detected.".into());
    }
    if lower.contains(":(){ :|:& };:") || lower.contains(":() { :|:& };:") {
        return Some("Fork-bomb pattern detected.".into());
    }
    let fetches = lower.contains("curl ") || lower.contains("wget ");
    let shells = lower.contains("| sh") || lower.contains("| bash") || lower.contains("| zsh");
    if fetches && shells {
        return Some("Remote fetch piped to a shell detected.".into());
    }
    if lower.contains("/etc/passwd") && (lower.contains('>') || lower.contains("tee ")) {
        return Some("Write to /etc/passwd detected.".into());
    }
    if compact.contains("shutdown ")
        || compact == "shutdown"
        || compact.contains("reboot ")
        || compact == "reboot"
        || compact.contains("halt ")
        || compact == "halt"
    {
        return Some("Host shutdown command detected.".into());
    }
    None
}

pub(super) fn tier_flags(tier: ToolTier) -> (bool, bool) {
    match tier {
        ToolTier::Read => (false, false),
        ToolTier::Write => (true, false),
        ToolTier::Exec => (false, true),
    }
}

pub(super) fn prompt_or_deny(tool: &str, detail: &str, hooks: &RunHooks) -> ToolDecision {
    if hooks.approve(tool, detail) {
        let (allow_write, allow_command) = tier_flags(tool_tier(tool));
        ToolDecision::Allow {
            allow_write,
            allow_command,
        }
    } else {
        ToolDecision::Deny(format!("tool approval denied for {}", tool))
    }
}

pub(super) fn resolve_tool_approval(
    args: &Args,
    tool: &str,
    input: &Value,
    hooks: &RunHooks,
) -> ToolDecision {
    let state = read_mode_state(&args.cwd);
    let tier = tool_tier(tool);
    let policy = tool_policy(&state, tool);
    let mode = approval_mode(args, &state);
    let safety = safety_override_reason(tool, input);

    // Plan mode is read-only by design: write- and exec-tier tools are denied
    // outright, with a hint on how to allow modifications again.
    if tier != ToolTier::Read
        && state
            .pointer("/plan/enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return ToolDecision::Deny(format!(
            "tool {tool} is blocked (plan mode is on; /plan off to allow modifications)"
        ));
    }

    if mode == ApprovalMode::Yolo {
        return match policy {
            Some(ToolPolicy::Deny) => {
                ToolDecision::Deny(format!("tool denied by policy: {}", tool))
            }
            Some(ToolPolicy::Prompt) => {
                prompt_or_deny(tool, safety.as_deref().unwrap_or(""), hooks)
            }
            Some(ToolPolicy::Allow) | None => {
                let (allow_write, allow_command) = tier_flags(tier);
                ToolDecision::Allow {
                    allow_write,
                    allow_command,
                }
            }
        };
    }

    if let Some(reason) = safety.as_deref() {
        return match policy {
            Some(ToolPolicy::Deny) => {
                ToolDecision::Deny(format!("tool denied by policy: {}", tool))
            }
            _ => prompt_or_deny(tool, reason, hooks),
        };
    }

    match policy {
        Some(ToolPolicy::Allow) => {
            let (allow_write, allow_command) = tier_flags(tier);
            return ToolDecision::Allow {
                allow_write,
                allow_command,
            };
        }
        Some(ToolPolicy::Deny) => {
            return ToolDecision::Deny(format!("tool denied by policy: {}", tool))
        }
        Some(ToolPolicy::Prompt) => return prompt_or_deny(tool, "", hooks),
        None => {}
    }

    if (tier == ToolTier::Write && args.allow_write)
        || (tier == ToolTier::Exec && args.allow_command)
    {
        let (allow_write, allow_command) = tier_flags(tier);
        return ToolDecision::Allow {
            allow_write,
            allow_command,
        };
    }

    let auto_allowed = match mode {
        ApprovalMode::AlwaysAsk => tier == ToolTier::Read,
        ApprovalMode::Write => matches!(tier, ToolTier::Read | ToolTier::Write),
        ApprovalMode::Yolo => true,
    };
    if auto_allowed {
        let (allow_write, allow_command) = tier_flags(tier);
        ToolDecision::Allow {
            allow_write,
            allow_command,
        }
    } else {
        prompt_or_deny(tool, "", hooks)
    }
}
