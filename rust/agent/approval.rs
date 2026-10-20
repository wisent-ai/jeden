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
    /// The mode a session runs in when neither config nor flags name one:
    /// older Jeden sessions only auto-ran write or command tools with
    /// explicit flags, and a missing setting keeps that gate.
    pub(crate) const DEFAULT: ApprovalMode = ApprovalMode::AlwaysAsk;
    pub(crate) fn name(self) -> String {
        crate::vocabulary::spelling(self)
    }
    /// Every mode's spelling, joined with `|` for a usage line.
    pub(crate) fn choices() -> String {
        Self::ALL
            .iter()
            .map(|mode| mode.name())
            .collect::<Vec<_>>()
            .join("|")
    }
    /// Every mode's spelling, kept for the process, for the settings schema
    /// that names its choices as static strings.
    pub(crate) fn spellings() -> &'static [&'static str] {
        static NAMES: std::sync::LazyLock<Vec<String>> =
            std::sync::LazyLock::new(|| ApprovalMode::ALL.iter().map(|mode| mode.name()).collect());
        static SPELLINGS: std::sync::LazyLock<Vec<&'static str>> =
            std::sync::LazyLock::new(|| NAMES.iter().map(String::as_str).collect());
        &SPELLINGS
    }
    /// [`Self::DEFAULT`] as the settings schema's JSON default.
    pub(crate) fn default_json() -> &'static str {
        static DEFAULT_JSON: std::sync::LazyLock<String> =
            std::sync::LazyLock::new(|| Value::String(ApprovalMode::DEFAULT.name()).to_string());
        &DEFAULT_JSON
    }
}

impl ToolPolicy {
    pub(crate) const ALL: &'static [ToolPolicy] =
        &[ToolPolicy::Allow, ToolPolicy::Deny, ToolPolicy::Prompt];
    pub(crate) fn name(self) -> String {
        crate::vocabulary::spelling(self)
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

/// The approval mode a session runs in: `--yolo`, then the mode the mode
/// state records, then both allow flags, then [`ApprovalMode::DEFAULT`]. A
/// recorded mode that is no mode is refused with the accepted spellings, not
/// read as unset.
pub(super) fn approval_mode(args: &Args, state: &Value) -> Result<ApprovalMode, String> {
    if args.yolo {
        return Ok(ApprovalMode::Yolo);
    }
    match state.pointer("/tools/approvalMode") {
        Some(Value::String(recorded)) => {
            return crate::vocabulary::named(recorded)
                .map_err(|cause| format!("tools.approvalMode in the mode state: {cause}"));
        }
        Some(other) => {
            return Err(format!(
                "tools.approvalMode in the mode state is not a word: {other}"
            ))
        }
        None => {}
    }
    if args.allow_write && args.allow_command {
        return Ok(ApprovalMode::Yolo);
    }
    Ok(ApprovalMode::DEFAULT)
}

/// The policy the mode state records for `tool`, if it records one; a
/// recorded policy that is no policy is refused with the accepted spellings.
pub(super) fn tool_policy(state: &Value, tool: &str) -> Result<Option<ToolPolicy>, String> {
    let pointer = format!(
        "/tools/approval/{}",
        tool.replace('~', "~0").replace('/', "~1")
    );
    match state.pointer(&pointer) {
        Some(Value::String(recorded)) => crate::vocabulary::named(recorded)
            .map(Some)
            .map_err(|cause| format!("the {tool} policy in the mode state: {cause}")),
        Some(other) => Err(format!(
            "the {tool} policy in the mode state is not a word: {other}"
        )),
        None => Ok(None),
    }
}

/// Whether the mode state turns plan mode on; a recorded value that is no
/// boolean is refused rather than read as off.
fn plan_enabled(state: &Value) -> Result<bool, String> {
    match state.pointer("/plan/enabled") {
        Some(Value::Bool(enabled)) => Ok(*enabled),
        Some(other) => Err(format!(
            "plan.enabled in the mode state is not a boolean: {other}"
        )),
        None => Ok(false),
    }
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
    let read = tool_policy(&state, tool).and_then(|policy| {
        let mode = approval_mode(args, &state)?;
        Ok((policy, mode, plan_enabled(&state)?))
    });
    let (policy, mode, plan) = match read {
        Ok(read) => read,
        Err(cause) => return ToolDecision::Deny(format!("tool {tool} denied: {cause}")),
    };

    // Plan mode is read-only by design: write- and exec-tier tools are denied
    // outright, with a hint on how to allow modifications again.
    if tier != ToolTier::Read && plan {
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
