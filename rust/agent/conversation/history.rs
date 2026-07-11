use super::*;

impl Conversation {
    /// Drop all turns, keeping the system prompt — backs /clear and /new.
    pub(crate) fn reset(&mut self, cwd: &Path) -> Result<(), String> {
        self.messages = vec![json!({ "role": "system", "content": system_prompt_checked(cwd)? })];
        self.recorder = SessionRecorder::new(cwd);
        self.recorder.ensure()
    }

    /// Refresh the system prompt for a new working directory (keeps live turns)
    /// so tools/commands visible from the new cwd are reflected — backs /move.
    pub(crate) fn rebase(&mut self, cwd: &Path) -> Result<(), String> {
        if let Some(first) = self.messages.first_mut() {
            if first.get("role").and_then(Value::as_str) == Some("system") {
                first["content"] = json!(system_prompt_checked(cwd)?);
            }
        }
        self.recorder.set_cwd(cwd)
    }

    /// Replace the live history with prior user/assistant turns — backs /resume
    /// so a resumed session actually continues in-process.
    pub(crate) fn load_history(&mut self, cwd: &Path, mut turns: Vec<Value>) -> Result<(), String> {
        let needs_base_system = turns
            .first()
            .and_then(|message| message.get("_jedenNeedsBaseSystem"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if needs_base_system {
            if let Some(first) = turns.first_mut().and_then(Value::as_object_mut) {
                first.remove("_jedenNeedsBaseSystem");
            }
        }
        let messages = if !needs_base_system
            && turns
                .first()
                .and_then(|message| message.get("role"))
                .and_then(Value::as_str)
                == Some("system")
        {
            turns
        } else {
            let mut messages =
                vec![json!({ "role": "system", "content": system_prompt_checked(cwd)? })];
            messages.extend(turns);
            messages
        };
        self.messages = messages;
        self.recorder.record_context("resume_seed", &self.messages)
    }

    /// Fork: keep the current in-memory history but switch to a NEW session dir
    /// so subsequent turns record into a separate lineage — backs /fork as a
    /// real session split, not a mode-state label. Returns the new session path.
    pub(crate) fn fork(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        let parent_entry = self.recorder.active_leaf()?;
        self.recorder = SessionRecorder::child(cwd, parent, parent_entry);
        self.recorder.ensure()?;
        self.recorder.record_context("fork_seed", &self.messages)?;
        Ok(self.recorder.path())
    }

    /// Branch the exact live model window into a child ledger.
    pub(crate) fn branch(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        let parent_entry = self.recorder.active_leaf()?;
        self.recorder = SessionRecorder::child(cwd, parent, parent_entry);
        self.recorder.ensure()?;
        self.recorder
            .record_context("branch_seed", &self.messages)?;
        Ok(self.recorder.path())
    }
}
