use super::*;

mod action;
mod compaction;
mod history;
mod local_exec;
mod turn;

pub(super) use action::{
    action_or_text, action_to_value, record_unexecuted_tool_action, run_tool_action,
};

/// A persistent agent conversation. In interactive mode one `Conversation`
/// lives for the whole session so each turn sees the full prior history (real
/// chat memory); the CLI one-shot builds a transient one per invocation.
pub(crate) struct Conversation {
    pub(super) messages: Vec<Value>,
    pub(super) recorder: SessionRecorder,
}

impl Conversation {
    pub(crate) fn new(cwd: &Path) -> Result<Self, String> {
        let mut recorder = SessionRecorder::new(cwd);
        recorder.ensure()?;
        Ok(Self {
            messages: vec![json!({ "role": "system", "content": system_prompt_checked(cwd)? })],
            recorder,
        })
    }

    pub(crate) fn new_model_only(cwd: &Path) -> Result<Self, String> {
        let mut recorder = SessionRecorder::new(cwd);
        recorder.ensure()?;
        Ok(Self {
            messages: vec![json!({
                "role": "system",
                "content": "You are Jeden in model-only mode. Follow the user request directly and do not call tools."
            })],
            recorder,
        })
    }

    pub(crate) fn open(cwd: &Path, session_dir: &Path) -> Result<Self, String> {
        let turns = crate::cli::sessions::session_conversation_turns(session_dir)?;
        let messages = history::normalized_history(cwd, turns)?;
        let recorder = SessionRecorder::open(cwd, session_dir)?;
        Ok(Self { messages, recorder })
    }

    pub(crate) fn session_path(&self) -> PathBuf {
        self.recorder.path()
    }

    /// Rough token estimate (~4 chars/token) over the live message window, for
    /// the status line. Not billing-accurate; a live signal, not a guess.
    pub(crate) fn approx_tokens(&self) -> usize {
        let chars: usize = self
            .messages
            .iter()
            .map(|m| m.to_string().chars().count())
            .sum();
        chars / 4
    }

    /// Number of non-system messages currently held.
    pub(crate) fn turn_len(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
            .count()
    }

    pub(super) fn auto_compaction_threshold() -> Option<usize> {
        if let Some(threshold) = env_usize("JEDEN_COMPACTION_THRESHOLD") {
            return Some(threshold);
        }
        let limit = env_usize("JEDEN_CONTEXT_LIMIT")?;
        // Compact before the context limit, reserving the larger of 15% of the
        // window or the configured token reserve. Clamp tiny test and development
        // windows to 85% when the reserve would consume the entire window.
        let reserve = env_usize("JEDEN_COMPACTION_RESERVE").unwrap_or(16_384);
        let fifteen_percent = ((limit as f64) * 0.15).ceil() as usize;
        let margin = std::cmp::max(fifteen_percent, reserve);
        if margin >= limit {
            Some(std::cmp::max(1, ((limit as f64) * 0.85).floor() as usize))
        } else {
            Some(limit - margin)
        }
    }

    pub(super) fn tool_result_tokens(content: &str) -> Option<usize> {
        let value: Value = serde_json::from_str(content).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("tool_result") {
            return None;
        }
        Some(std::cmp::max(1, content.chars().count() / 4))
    }

    pub(super) fn prune_tool_results_if_needed(
        &mut self,
        threshold: usize,
    ) -> Result<usize, String> {
        if self.approx_tokens() < threshold {
            return Ok(0);
        }
        let protect_tokens = env_usize("JEDEN_TOOL_PRUNE_PROTECT_TOKENS").unwrap_or(40_000);
        let min_savings = env_usize("JEDEN_TOOL_PRUNE_MIN_SAVINGS_TOKENS").unwrap_or(20_000);
        let min_tool_tokens = env_usize("JEDEN_TOOL_PRUNE_MIN_TOOL_TOKENS").unwrap_or(50);
        let mut protected_tokens = 0usize;
        let mut protected_latest = false;
        let mut candidates = Vec::new();
        for (idx, message) in self.messages.iter().enumerate().rev() {
            let Some(content) = message.get("content").and_then(Value::as_str) else {
                continue;
            };
            let Some(tokens) = Self::tool_result_tokens(content) else {
                continue;
            };
            if !protected_latest {
                protected_latest = true;
                protected_tokens = protected_tokens.saturating_add(tokens);
                continue;
            }
            if protected_tokens < protect_tokens {
                protected_tokens = protected_tokens.saturating_add(tokens);
                continue;
            }
            if tokens >= min_tool_tokens {
                candidates.push((idx, tokens));
            }
        }
        let needed_savings = self
            .approx_tokens()
            .saturating_sub(threshold)
            .saturating_add(1);
        let target_savings = std::cmp::max(needed_savings, min_savings);
        let potential_savings: usize = candidates.iter().map(|(_, tokens)| *tokens).sum();
        if potential_savings < target_savings {
            return Ok(0);
        }
        candidates.sort_by_key(|(idx, _)| *idx);
        let mut selected = Vec::new();
        let mut saved = 0usize;
        for (idx, tokens) in candidates {
            if saved >= target_savings {
                break;
            }
            saved = saved.saturating_add(tokens);
            selected.push((idx, tokens));
        }
        for (idx, tokens) in &selected {
            let replacement = json!({"type": "tool_result", "result": format!("[Output truncated - {} tokens]", tokens)}).to_string();
            self.messages[*idx]["content"] = json!(replacement);
        }
        self.recorder.record(
            "tool_prune",
            json!({ "pruned": selected.len(), "savedTokensApprox": saved, "threshold": threshold }),
        )?;
        Ok(saved)
    }
}
