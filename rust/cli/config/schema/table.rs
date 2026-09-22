//! Every setting this product has, with what it accepts and what it defaults
//! to.
//!
//! Split out of `cli/config/schema.rs`, which had grown past the module line
//! cap.

use super::SettingSpec;

/// `auto` plus every pinnable language, from the declaration the parser
/// reads. The schema used to carry its own copy of all sixty-five codes,
/// so a language added to the pinnable set appeared in `config set` and
/// not in `config list`, or the other way round.
///
/// The declaration is read at first use and kept for the process, because
/// a `SettingSpec` names its choices as `&'static [&'static str]`.
fn ui_language_choices() -> &'static [&'static str] {
    static CHOICES: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        let mut choices = vec![UI_LANGUAGE_AUTO];
        choices.extend(
            super::ui_language_codes()
                .iter()
                .map(|code| code.as_str() as &'static str),
        );
        choices
    });
    &CHOICES
}

pub(super) static SETTINGS_SCHEMA: std::sync::LazyLock<Vec<SettingSpec>> = std::sync::LazyLock::new(|| {
    vec![
    SettingSpec {
        key: "model",
        typ: "string",
        description: "The model route turns run on, as Brama advertises it; /setup records it here, and JEDEN_MODEL or --model overrides it for one run. Empty means no model is selected.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: "tools.approvalMode",
        typ: "enum",
        description: "Default approval policy for tool execution.",
        default_json: "\"always-ask\"",
        enum_values: &["always-ask", "write", "yolo"],
    },
    SettingSpec {
        key: "commands.enableClaudeUser",
        typ: "boolean",
        description: "Enable user slash commands from ~/.claude/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "commands.enableClaudeProject",
        typ: "boolean",
        description: "Enable project slash commands from .claude/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "commands.enableOpencodeUser",
        typ: "boolean",
        description: "Enable user slash commands from ~/.config/opencode/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "commands.enableOpencodeProject",
        typ: "boolean",
        description: "Enable project slash commands from .opencode/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "startup.showSplash",
        typ: "boolean",
        description: "Show the startup splash animation on normal launches.",
        default_json: "false",
        enum_values: &[],
    },
    SettingSpec {
        key: "startup.quiet",
        typ: "boolean",
        description: "Suppress startup chrome including the splash.",
        default_json: "false",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.maxBytes",
        typ: "number",
        description: "Maximum UTF-8 bytes loaded from discovered context and rule files.",
        default_json: "131072",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.maxTokens",
        typ: "number",
        description: "Approximate token budget for discovered context and rule files.",
        default_json: "32768",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.enabled",
        typ: "boolean",
        description: "Append context recommendations to every task prompt.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.limit",
        typ: "number",
        description: "How many context recommendations one answer carries.",
        default_json: "6",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.maxChars",
        typ: "number",
        description: "Character budget for the recommendation block added to a prompt.",
        default_json: "6000",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.sources",
        typ: "string",
        description: "Context sources consulted on every run: any comma-separated subset of files, ground-truth, memory, transcripts, or all. Sources run concurrently and each runs to completion, so a slow source is time the turn pays.",
        default_json: "\"files,memory\"",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.roots",
        typ: "string",
        description: "Colon-separated roots the files source reads, each optionally suffixed @depth. Empty means the project and ~/.jeden.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.fileExtensions",
        typ: "string",
        description: "Comma-separated file extensions the files source reads. Empty, the default, reads every readable text file: code, configuration and prose alike.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.groundTruthUrl",
        typ: "string",
        description: "Base URL of the Wisent ground-truth API. Empty means WISENT_GROUND_TRUTH_API or GROUND_TRUTH_API decides.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.advisor.transcriptLakeBin",
        typ: "string",
        description: "Transcript Lake executable used by the transcripts source. Empty means transcript-lake on PATH.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: COMMUNICATION_CONTRACT_KEY,
        typ: "string",
        description: "How Jeden writes to you. Empty means Jeden's default: plain language, then three parts — what was done, blockers, next steps. Write your own text to replace it, or 'none' to add no communication instruction.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: FUNCTIONALITY_CONTRACT_KEY,
        typ: "string",
        description: "Instructions for how Jeden carries out work and what it must complete before answering.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: COMMUNICATION_MODE_KEY,
        typ: "enum",
        description: "Communication mode: normal shows tool names while working, debug also shows each tool call with its input, every tool result, and the model's reasoning, quiet shows only the answer.",
        default_json: "\"normal\"",
        enum_values: &CommunicationMode::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_TOOL_CALLS_KEY,
        typ: "enum",
        description: "Show the model's tool calls; auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: &Visibility::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_TOOL_RESULTS_KEY,
        typ: "enum",
        description: "Show what each tool returned; auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: &Visibility::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_REASONING_KEY,
        typ: "enum",
        description: "Show the model's reasoning when the route streams it; auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: &Visibility::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_CODE_KEY,
        typ: "enum",
        description: "Show code blocks in answers; hide replaces each block with a placeholder and asks the model to answer in prose. Auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: &Visibility::VALUES,
    },
    SettingSpec {
        key: "rules.alwaysApply",
        typ: "array",
        description: "Typed sticky rules injected into every rebuilt system prompt.",
        default_json: "[]",
        enum_values: &[],
    },
    SettingSpec {
        key: "hooks.tamaRegistry",
        typ: "string",
        description: "Path to the Tama hook registry (shared-hooks registry.json). Empty disables Tama hooks; unset auto-discovers known locations.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: "secrets.mode",
        typ: "enum",
        description: "Protect known secrets in model-bound text by redaction or obfuscation.",
        default_json: "\"redact\"",
        enum_values: &["redact", "obfuscate"],
    },
    SettingSpec {
        key: "secrets.minLength",
        typ: "number",
        description: "Minimum length for automatically discovered environment secrets.",
        default_json: "8",
        enum_values: &[],
    },
    SettingSpec {
        key: "secrets.discoverEnvironment",
        typ: "boolean",
        description: "Automatically protect values from secret-named environment variables.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "ui.language",
        typ: "enum",
        description: "Conversation language: auto follows the user's messages; an ISO 639 code pins the answer language (65 languages as in wisent-app).",
        default_json: "\"auto\"",
        enum_values: ui_language_choices(),
    },
    SettingSpec {
        key: "ui.theme",
        typ: "enum",
        description: "Color theme: a named preset, 'custom' to load .jeden/theme.json, or 'auto' (graphite-dark).",
        default_json: "\"auto\"",
        enum_values: &[
            "auto",
            "graphite-dark",
            "paper-light",
            "titanium",
            "nord",
            "color-blind",
            "mono",
            "high-contrast",
            "custom",
        ],
    },
    ]
});
