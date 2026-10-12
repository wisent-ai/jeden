use super::*;
pub(crate) use crate::tools::ToolTier;

/// How much a session runs before it asks; spelled in config and on the
/// command line by its serde name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ApprovalMode {
    AlwaysAsk,
    Write,
    Yolo,
}

/// What a per-tool policy says about one tool, spelled by its serde name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ToolPolicy {
    Allow,
    Deny,
    Prompt,
}

impl ApprovalMode {
    pub(crate) const ALL: &'static [ApprovalMode] = &[
        ApprovalMode::AlwaysAsk,
        ApprovalMode::Write,
        ApprovalMode::Yolo,
    ];
    pub(crate) fn name(self) -> String {
        spelling(self)
    }
    /// Every mode's spelling, joined with `|` for a usage line.
    pub(crate) fn choices() -> String {
        Self::ALL
            .iter()
            .map(|mode| mode.name())
            .collect::<Vec<_>>()
            .join("|")
    }
}

impl ToolPolicy {
    pub(crate) const ALL: &'static [ToolPolicy] =
        &[ToolPolicy::Allow, ToolPolicy::Deny, ToolPolicy::Prompt];
    pub(crate) fn name(self) -> String {
        spelling(self)
    }
    /// Every policy's spelling, joined with `|` for a usage line.
    pub(crate) fn choices() -> String {
        Self::ALL
            .iter()
            .map(|policy| policy.name())
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// The value of a declared vocabulary spelled `name`, or nothing.
pub(crate) fn named<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(Value::String(name.trim().to_owned())).ok()
}

/// How a declared vocabulary value is spelled.
pub(crate) fn spelling<T: serde::Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        other => unreachable!("a unit variant serializes as its name, not {other:?}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolDecision {
    Allow {
        allow_write: bool,
        allow_command: bool,
    },
    Deny(String),
}

/// The tier a tool acts at: what the builtin catalog declares for it; an
/// MCP server's tool, which the catalog cannot know, changes what its server
/// holds; and anything else, a custom or future tool, is treated as running
/// code until it is declared.
pub(super) fn tool_tier(tool: &str) -> ToolTier {
    if let Some(tier) = crate::tools::builtin_tier(tool) {
        return tier;
    }
    if tool.starts_with("mcp_") {
        return ToolTier::Write;
    }
    ToolTier::Exec
}

pub(super) fn parse_approval_mode(value: &str) -> Option<ApprovalMode> {
    named(value)
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
    named(value)
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

/// A mode state that cannot be read denies the tool with the file and the
/// cause: the approval policy and plan mode it holds are unknown, and reading
/// it as empty would allow what plan mode or a tool policy forbids.
pub(super) fn resolve_tool_approval(args: &Args, tool: &str, hooks: &RunHooks) -> ToolDecision {
    let state = match read_mode_state(&args.cwd) {
        Ok(state) => state,
        Err(cause) => return ToolDecision::Deny(format!("tool {tool} denied: {cause}")),
    };
    let tier = tool_tier(tool);
    let policy = tool_policy(&state, tool);
    let mode = approval_mode(args, &state);

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
            Some(ToolPolicy::Prompt) => prompt_or_deny(tool, "", hooks),
            Some(ToolPolicy::Allow) | None => {
                let (allow_write, allow_command) = tier_flags(tier);
                ToolDecision::Allow {
                    allow_write,
                    allow_command,
                }
            }
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
