//! Communication modes: what a human sees of a turn, resolved from config.
//!
//! `communication.mode` is a preset and the four `communication.*` visibility
//! keys override one item each. The resolved [`DisplayPolicy`] is applied where
//! turn events leave the agent — the interactive terminal bridge and the SDK
//! session that feeds RPC, headless, and Jeden Desktop — so every surface shows
//! the same thing. The session transcript always records everything.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// Preset that decides both which items are shown and how much detail they carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CommunicationMode {
    /// Tool names while working, the answer with its code; no results, no reasoning.
    #[default]
    Normal,
    /// Every tool call with its input, every result, the model's reasoning, code.
    Debug,
    /// Only the answer.
    Quiet,
}

impl CommunicationMode {
    pub(crate) const VALUES: &'static [&'static str] = &["normal", "debug", "quiet"];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Debug => "debug",
            Self::Quiet => "quiet",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" => Some(Self::Normal),
            "debug" => Some(Self::Debug),
            "quiet" => Some(Self::Quiet),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for CommunicationMode {
    /// A value the file cannot name falls back to the preset default instead of
    /// discarding the whole configuration.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::parse(&raw).unwrap_or_default())
    }
}

/// Per-item override: `auto` follows the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Visibility {
    #[default]
    Auto,
    Show,
    Hide,
}

impl Visibility {
    pub(crate) const VALUES: &'static [&'static str] = &["auto", "show", "hide"];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "show" => Some(Self::Show),
            "hide" => Some(Self::Hide),
            _ => None,
        }
    }

    fn resolve(self, mode_default: bool) -> bool {
        match self {
            Self::Auto => mode_default,
            Self::Show => true,
            Self::Hide => false,
        }
    }
}

impl<'de> Deserialize<'de> for Visibility {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::parse(&raw).unwrap_or_default())
    }
}

/// The `communication` block of the layered configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct CommunicationConfig {
    #[serde(default)]
    pub(crate) mode: CommunicationMode,
    #[serde(rename = "toolCalls", default)]
    pub(crate) tool_calls: Visibility,
    #[serde(rename = "toolResults", default)]
    pub(crate) tool_results: Visibility,
    #[serde(default)]
    pub(crate) reasoning: Visibility,
    #[serde(default)]
    pub(crate) code: Visibility,
}

/// What the current turn shows, with every `auto` resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPolicy {
    pub(crate) mode: CommunicationMode,
    /// The model's tool calls: names while working in every mode, inputs in debug.
    pub(crate) tool_calls: bool,
    /// What each tool returned, as a bounded preview.
    pub(crate) tool_results: bool,
    /// The model's reasoning stream, when the route serves one.
    pub(crate) reasoning: bool,
    /// Fenced code blocks in answers; hidden blocks become a placeholder line.
    pub(crate) code: bool,
}

impl Default for DisplayPolicy {
    fn default() -> Self {
        Self::resolve(&CommunicationConfig::default())
    }
}

/// The note a turn emits while a tool runs, and the only note a hidden tool
/// call is allowed to replace.
const TOOL_NOTE_PREFIX: &str = "tool: ";
const HIDDEN_TOOL_NOTE: &str = "working…";

impl DisplayPolicy {
    pub(crate) fn resolve(config: &CommunicationConfig) -> Self {
        let (tool_calls, tool_results, reasoning, code) = match config.mode {
            CommunicationMode::Normal => (true, false, false, true),
            CommunicationMode::Debug => (true, true, true, true),
            CommunicationMode::Quiet => (false, false, false, true),
        };
        Self {
            mode: config.mode,
            tool_calls: config.tool_calls.resolve(tool_calls),
            tool_results: config.tool_results.resolve(tool_results),
            reasoning: config.reasoning.resolve(reasoning),
            code: config.code.resolve(code),
        }
    }

    /// The policy in force for a turn started in `cwd`, read fresh so a change
    /// saved from the CLI or Jeden Desktop applies to the next turn.
    pub(crate) fn for_cwd(cwd: &Path) -> Self {
        Self::resolve(&crate::load_config(cwd).communication)
    }

    /// Tool calls carry their inputs only in debug mode; elsewhere a shown tool
    /// call is its name while it runs.
    pub(crate) fn tool_call_detail(&self) -> bool {
        self.tool_calls && self.mode == CommunicationMode::Debug
    }

    /// A progress note as the operator may see it. Hiding tool calls replaces
    /// the tool-name note with a neutral one; refusals and everything else pass.
    pub(crate) fn note<'a>(&self, note: &'a str) -> &'a str {
        if !self.tool_calls && note.starts_with(TOOL_NOTE_PREFIX) {
            HIDDEN_TOOL_NOTE
        } else {
            note
        }
    }

    /// An answer as the operator may see it.
    pub(crate) fn answer(&self, text: String) -> String {
        if self.code {
            text
        } else {
            hide_code_blocks(&text)
        }
    }

    /// The resolved booleans, for clients that render history themselves.
    pub(crate) fn json(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "toolCalls": self.tool_calls,
            "toolCallDetail": self.tool_call_detail(),
            "toolResults": self.tool_results,
            "reasoning": self.reasoning,
            "code": self.code,
        })
    }
}

/// Replace every fenced code block in `text` with a one-line placeholder.
pub(crate) fn hide_code_blocks(text: &str) -> String {
    let mut filter = CodeFilter::default();
    let mut out = filter.push(text);
    out.push_str(&filter.finish());
    out
}

/// Streaming fence filter. Text outside ``` / ~~~ fences passes through as
/// soon as its line is complete; fenced lines are swallowed and the closing
/// fence emits `[code hidden: N lines]`. Feed pieces with [`CodeFilter::push`]
/// and drain the tail with [`CodeFilter::finish`].
#[derive(Default)]
pub(crate) struct CodeFilter {
    pending: String,
    fence: Option<Fence>,
    hidden_lines: usize,
}

#[derive(Clone, Copy)]
struct Fence {
    marker: char,
    length: usize,
}

impl CodeFilter {
    pub(crate) fn push(&mut self, piece: &str) -> String {
        self.pending.push_str(piece);
        let mut out = String::new();
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending.drain(..=newline).collect::<String>();
            self.consume_line(&line, &mut out);
        }
        out
    }

    pub(crate) fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.consume_line(&line, &mut out);
        }
        if self.fence.take().is_some() {
            out.push_str(&placeholder(self.hidden_lines));
            self.hidden_lines = 0;
        }
        out
    }

    fn consume_line(&mut self, line: &str, out: &mut String) {
        let content = line.trim_start_matches(' ');
        match self.fence {
            Some(fence) => {
                if closes_fence(content, fence) {
                    self.fence = None;
                    out.push_str(&placeholder(self.hidden_lines));
                    if line.ends_with('\n') {
                        out.push('\n');
                    }
                    self.hidden_lines = 0;
                } else {
                    self.hidden_lines += 1;
                }
            }
            None => match opens_fence(content) {
                Some(fence) => {
                    self.fence = Some(fence);
                    self.hidden_lines = 0;
                }
                None => out.push_str(line),
            },
        }
    }
}

fn placeholder(lines: usize) -> String {
    format!(
        "[code hidden: {lines} line{}]",
        if lines == 1 { "" } else { "s" }
    )
}

fn fence_run(content: &str) -> Option<Fence> {
    let marker = content.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = content.chars().take_while(|c| *c == marker).count();
    (length >= 3).then_some(Fence { marker, length })
}

fn opens_fence(content: &str) -> Option<Fence> {
    let fence = fence_run(content)?;
    let info = &content[fence.length..];
    // A backtick fence cannot carry backticks in its info string.
    (fence.marker != '`' || !info.contains('`')).then_some(fence)
}

fn closes_fence(content: &str, open: Fence) -> bool {
    match fence_run(content) {
        Some(run) if run.marker == open.marker && run.length >= open.length => {
            content[run.length..].trim().is_empty()
        }
        _ => false,
    }
}
