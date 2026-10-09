use super::*;

pub mod protocol;

mod compaction;
mod completion;
mod history;
mod local_exec;
mod turn;

pub(super) use protocol::action::{
    action_or_text, action_to_value, record_unexecuted_tool_action, run_tool_action,
};

/// A persistent agent conversation. In interactive mode one `Conversation`
/// lives for the whole session so each turn sees the full prior history (real
/// chat memory); the CLI one-shot builds a transient one per invocation.
pub(crate) struct Conversation {
    pub(super) messages: Vec<Value>,
    pub(super) recorder: SessionRecorder,
    pub(super) manages_completion: bool,
    pub(super) inspection: bool,
    pub(super) continuation: bool,
    pub(super) reconcile_completion: bool,
    pub(super) session_context_pending: Option<&'static str>,
}

impl Conversation {
    pub(crate) fn new(cwd: &Path) -> Result<Self, String> {
        let mut recorder = SessionRecorder::new(cwd);
        recorder.ensure()?;
        Ok(Self {
            messages: vec![json!({ "role": "system", "content": system_prompt_checked(cwd)? })],
            recorder,
            manages_completion: true,
            inspection: false,
            continuation: false,
            reconcile_completion: false,
            session_context_pending: Some("startup"),
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
            manages_completion: false,
            inspection: false,
            continuation: false,
            reconcile_completion: false,
            session_context_pending: None,
        })
    }

    pub(crate) fn open(cwd: &Path, session_dir: &Path) -> Result<Self, String> {
        let turns = crate::cli::sessions::session_conversation_turns(session_dir)?;
        let messages = history::normalized_history(cwd, turns)?;
        let recorder = SessionRecorder::open(cwd, session_dir)?;
        Ok(Self {
            messages,
            recorder,
            manages_completion: true,
            inspection: false,
            continuation: false,
            reconcile_completion: true,
            session_context_pending: Some("resume"),
        })
    }

    /// Pursuit owns its stage acceptance and must not recursively start another controller.
    pub(crate) fn new_stage(cwd: &Path) -> Result<Self, String> {
        let mut conversation = Self::new(cwd)?;
        conversation.manages_completion = false;
        Ok(conversation)
    }

    /// Restore a Pursuit-owned stage without starting another acceptance controller.
    pub(crate) fn open_stage(cwd: &Path, session_dir: &Path) -> Result<Self, String> {
        let mut conversation = Self::open(cwd, session_dir)?;
        conversation.manages_completion = false;
        conversation.reconcile_completion = false;
        conversation.continuation = true;
        Ok(conversation)
    }

    fn new_inspection(cwd: &Path) -> Result<Self, String> {
        let mut conversation = Self::new_stage(cwd)?;
        conversation.inspection = true;
        Ok(conversation)
    }

    pub(crate) fn session_path(&self) -> PathBuf {
        self.recorder.path()
    }

    /// Restore lifecycle context in the shared model window for every client.
    /// Leave the transition pending on failure so a later turn cannot skip it.
    pub(super) fn restore_session_context(&mut self, args: &Args) -> Result<(), String> {
        if args.model_only {
            return Ok(());
        }
        let Some(source) = self.session_context_pending else {
            return Ok(());
        };
        let session = self.recorder.path();
        let session_id = session.file_name().and_then(|name| name.to_str()).ok_or_else(|| {
            format!("SessionStart cannot identify session directory {}", session.display())
        })?;
        let context = crate::hooks::session_start(
            &args.cwd,
            args.allow_command,
            source,
            session_id,
            &session.join("transcript.jsonl"),
        ).map_err(|reason| format!("SessionStart ({source}) for {session_id}: {reason}"))?;
        let before = self.messages.len();
        if !context.trim().is_empty() {
            self.messages.push(json!({ "role": "system", "content": context }));
        }
        if let Err(reason) = self.recorder.record_context("session_start", &self.messages) {
            self.messages.truncate(before);
            return Err(format!("SessionStart ({source}) context persistence: {reason}"));
        }
        self.session_context_pending = None;
        Ok(())
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

    /// The token count at which automatic compaction runs: the stated
    /// `JEDEN_COMPACTION_THRESHOLD`, or the stated `JEDEN_CONTEXT_LIMIT` less
    /// the stated `JEDEN_COMPACTION_RESERVE`. With neither, compaction is the
    /// operator's `/compact` alone; a limit without its reserve is refused by
    /// name rather than given a share of the window nobody chose.
    pub(super) fn auto_compaction_threshold() -> Result<Option<usize>, String> {
        if let Some(threshold) = env_usize("JEDEN_COMPACTION_THRESHOLD") {
            return Ok(Some(threshold));
        }
        let Some(limit) = env_usize("JEDEN_CONTEXT_LIMIT") else {
            return Ok(None);
        };
        let reserve = env_usize("JEDEN_COMPACTION_RESERVE").ok_or_else(|| {
            format!(
                "JEDEN_CONTEXT_LIMIT is {limit} but JEDEN_COMPACTION_RESERVE is not set: state how many tokens compaction keeps free, or state JEDEN_COMPACTION_THRESHOLD"
            )
        })?;
        if reserve >= limit {
            return Err(format!(
                "JEDEN_COMPACTION_RESERVE {reserve} leaves nothing of JEDEN_CONTEXT_LIMIT {limit} before compaction"
            ));
        }
        Ok(Some(limit - reserve))
    }

    /// The three stated bounds of tool-result pruning, or `None` when none is
    /// stated (pruning is then not part of compaction). Some but not all is
    /// refused by name.
    fn tool_prune_bounds() -> Result<Option<(usize, usize, usize)>, String> {
        let names = [
            "JEDEN_TOOL_PRUNE_PROTECT_TOKENS",
            "JEDEN_TOOL_PRUNE_MIN_SAVINGS_TOKENS",
            "JEDEN_TOOL_PRUNE_MIN_TOOL_TOKENS",
        ];
        let values = names.map(env_usize);
        match values {
            [Some(protect), Some(min_savings), Some(min_tool)] => Ok(Some((protect, min_savings, min_tool))),
            [None, None, None] => Ok(None),
            _ => {
                let missing: Vec<&str> = names
                    .iter()
                    .zip(values)
                    .filter(|(_, value)| value.is_none())
                    .map(|(name, _)| *name)
                    .collect();
                Err(format!(
                    "tool-result pruning needs all of {}; {} not set",
                    names.join(", "),
                    missing.join(", ")
                ))
            }
        }
    }

    pub(super) fn tool_result_tokens(content: &str) -> Option<usize> {
        let value: Value = serde_json::from_str(content).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("tool_result") {
            return None;
        }
        Some(std::cmp::max(1, content.chars().count() / 4))
    }

    pub(super) fn prune_tool_results_if_needed(&mut self, threshold: usize) -> Result<(), String> {
        if self.approx_tokens() < threshold {
            return Ok(());
        }
        let Some((protect_tokens, min_savings, min_tool_tokens)) = Self::tool_prune_bounds()? else {
            return Ok(());
        };
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
            return Ok(());
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
        Ok(())
    }
}
