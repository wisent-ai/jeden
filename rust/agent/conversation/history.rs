use super::*;

impl Conversation {
    /// Drop all turns, keeping the system prompt — backs /clear and /new.
    pub(crate) fn reset(&mut self, cwd: &Path) -> Result<(), String> {
        self.messages = vec![json!({ "role": "system", "content": system_prompt(cwd) })];
        self.recorder = SessionRecorder::new(cwd);
        self.recorder.ensure()
    }

    /// Refresh the system prompt for a new working directory (keeps live turns)
    /// so tools/commands visible from the new cwd are reflected — backs /move.
    pub(crate) fn rebase(&mut self, cwd: &Path) -> Result<(), String> {
        if let Some(first) = self.messages.first_mut() {
            if first.get("role").and_then(Value::as_str) == Some("system") {
                first["content"] = json!(system_prompt(cwd));
            }
        }
        self.recorder.set_cwd(cwd)
    }

    /// Replace the live history with prior user/assistant turns — backs /resume
    /// so a resumed session actually continues in-process.
    pub(crate) fn load_history(&mut self, cwd: &Path, turns: Vec<Value>) -> Result<(), String> {
        let mut messages = vec![json!({ "role": "system", "content": system_prompt(cwd) })];
        // Also record the loaded turns into the live transcript so the resumed
        // context is part of this session's export and any later /branch off it.
        for turn in &turns {
            let role = turn.get("role").and_then(Value::as_str).unwrap_or("");
            let content = turn.get("content").and_then(Value::as_str).unwrap_or("");
            match role {
                "user" => { self.recorder.record("user", json!({ "task": content }))?; }
                "assistant" => { self.recorder.record("final", json!({ "text": content }))?; }
                _ => {}
            }
        }
        messages.extend(turns);
        self.messages = messages;
        Ok(())
    }

    /// Fork: keep the current in-memory history but switch to a NEW session dir
    /// so subsequent turns record into a separate lineage — backs /fork as a
    /// real session split, not a mode-state label. Returns the new session path.
    pub(crate) fn fork(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        self.recorder = SessionRecorder::new(cwd);
        self.recorder.ensure()?;
        self.recorder.record("fork", json!({ "parent": parent }))?;
        Ok(self.recorder.path())
    }

    /// Like [`fork`] but also replays the parent session's clean user/final
    /// turns into the new session's transcript so the branch is fully resumable
    /// with its prior context — backs a navigable `/branch`. Seeding from the
    /// parent transcript (not `self.messages`) avoids recording intermediate
    /// tool-action/tool-result messages as conversation turns.
    pub(crate) fn branch(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        let prior = crate::session_conversation_turns(&parent);
        self.recorder = SessionRecorder::new(cwd);
        self.recorder.ensure()?;
        self.recorder.record("fork", json!({ "parent": parent }))?;
        for turn in &prior {
            let role = turn.get("role").and_then(Value::as_str).unwrap_or("");
            let content = turn.get("content").and_then(Value::as_str).unwrap_or("");
            match role {
                "user" => { self.recorder.record("user", json!({ "task": content }))?; }
                "assistant" => { self.recorder.record("final", json!({ "text": content }))?; }
                _ => {}
            }
        }
        Ok(self.recorder.path())
    }
}
