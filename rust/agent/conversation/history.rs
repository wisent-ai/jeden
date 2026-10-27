use super::*;

pub(super) fn normalized_history(cwd: &Path, mut turns: Vec<Value>) -> Result<Vec<Value>, String> {
    // Only a history that carries the marker asks for the base system prompt.
    let needs_base_system = turns
        .first()
        .and_then(|message| message.get("_jedenNeedsBaseSystem"))
        .and_then(Value::as_bool)
        == Some(true);
    if needs_base_system {
        if let Some(first) = turns.first_mut().and_then(Value::as_object_mut) {
            first.remove("_jedenNeedsBaseSystem");
        }
    }
    if !needs_base_system
        && turns
            .first()
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            == Some("system")
    {
        Ok(turns)
    } else {
        let mut messages =
            vec![json!({ "role": "system", "content": system_prompt_checked(cwd)? })];
        messages.extend(turns);
        Ok(messages)
    }
}

/// Prepare durable child state before any live recorder or model window changes.
pub(super) fn prepare_child(
    cwd: &Path,
    parent: &Path,
    leaf: Option<String>,
    transition: &str,
) -> Result<SessionRecorder, String> {
    let mut recorder = SessionRecorder::child(cwd, parent.to_path_buf(), leaf)?;
    let destination = recorder.path();
    recorder.ensure().map_err(|reason| {
        format!(
            "{transition} could not prepare child session {}: {reason}",
            destination.display()
        )
    })?;
    crate::completion::inherit(parent, &destination).map_err(|reason| {
        format!(
            "{transition} could not inherit retained work from {} into {}: {reason}",
            parent.display(),
            destination.display()
        )
    })?;
    Ok(recorder)
}

impl Conversation {
    /// Clear the model window, not the operator's retained obligations.
    pub(crate) fn reset(&mut self, cwd: &Path) -> Result<(), String> {
        let messages = vec![json!({ "role": "system", "content": system_prompt_checked(cwd)? })];
        let parent = self.recorder.path();
        let leaf = self.recorder.active_leaf()?;
        let mut recorder = prepare_child(cwd, &parent, leaf, "reset")?;
        recorder.record_context("reset_seed", &messages)?;
        // The chat goes on in the new session directory: what was said so
        // far joins it before the window is cleared.
        self.leave_memory()?;
        self.recorder = recorder;
        self.messages = messages;
        self.reconcile_completion = true;
        self.session_context_pending = Some("reset");
        Ok(())
    }

    pub(crate) fn move_to(&mut self, target: &str) -> Result<PathBuf, String> {
        let target = target.trim();
        if target.is_empty() {
            return Err("Usage: /move <directory>".into());
        }
        let candidate = self.cwd().join(target);
        let resolved = candidate
            .canonicalize()
            .map_err(|error| format!("cannot move to {}: {error}", candidate.display()))?;
        if !resolved.is_dir() {
            return Err(format!("not a directory: {}", resolved.display()));
        }
        self.rebase(&resolved)?;
        Ok(resolved)
    }

    /// Refresh the system prompt for a new working directory (keeps live turns)
    /// so tools/commands visible from the new cwd are reflected — backs /move.
    pub(crate) fn rebase(&mut self, cwd: &Path) -> Result<(), String> {
        let prompt = self
            .messages
            .first()
            .filter(|first| first.get("role").and_then(Value::as_str) == Some("system"))
            .map(|_system_message| system_prompt_checked(cwd).map(|prompt| json!(prompt)))
            .transpose()?;
        self.recorder.set_cwd(cwd)?;
        if let Some(prompt) = prompt {
            if let Some(first) = self.messages.first_mut() {
                first["content"] = prompt;
            }
        }
        Ok(())
    }

    /// Resume the selected source, including its acceptance state and evidence lineage.
    pub(crate) fn load_history(
        &mut self,
        cwd: &Path,
        turns: Vec<Value>,
        source: &Path,
    ) -> Result<(), String> {
        let messages = normalized_history(cwd, turns)?;
        let leaf = crate::cli::sessions::session_active_leaf(source)?;
        let mut recorder = prepare_child(cwd, source, leaf, "resume")?;
        recorder.record_context("resume_seed", &messages)?;
        // The resumed child goes on in its chat, which already holds the
        // history the source logged.
        self.leave_memory()?;
        self.recorder = recorder;
        self.messages = messages;
        self.reconcile_completion = true;
        self.session_context_pending = Some("resume");
        Ok(())
    }

    pub(crate) fn checkpoint(&mut self, label: &str) -> Result<String, String> {
        let label = (!label.trim().is_empty()).then(|| label.trim().to_owned());
        let id = self.recorder.record_checkpoint(label, &self.messages)?;
        Ok(format!("Checkpoint {id} created."))
    }

    pub(crate) fn list_checkpoints(&self) -> Result<String, String> {
        crate::cli::sessions::list_checkpoint_entries(&self.recorder.path())
    }

    pub(crate) fn rewind(&mut self, checkpoint_id: &str) -> Result<String, String> {
        let checkpoint_id = checkpoint_id.trim();
        if checkpoint_id.is_empty() {
            return Err("Usage: /rewind <checkpoint-event-id>".into());
        }
        let (new_leaf, messages) = self.recorder.rewind(checkpoint_id)?;
        self.messages = messages;
        // The log keeps every message said before the rewind, and a note after
        // them names the ones taken back; what is said from the checkpoint on
        // follows the note. A process that never opened the memory (a
        // one-shot `/rewind`) writes the same note from the history as it
        // stands now.
        if let Some(memory) = &mut self.memory {
            memory.replaced(&self.messages)?;
        } else {
            crate::chat_memory::session::ChatMemory::rewound_unopened(
                &self.recorder.path(),
                self.cwd(),
                &self.messages,
            )?;
        }
        self.session_context_pending = Some("rewind");
        Ok(format!(
            "Rewound to checkpoint {checkpoint_id}; new active leaf is {new_leaf}."
        ))
    }

    /// Fork: keep the current in-memory history but switch to a NEW session dir
    /// so subsequent turns record into a separate lineage — backs /fork as a
    /// real session split, not a mode-state label. Returns the new session path.
    pub(crate) fn fork(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        let parent_entry = self.recorder.active_leaf()?;
        let mut recorder = prepare_child(cwd, &parent, parent_entry, "fork")?;
        recorder.record_context("fork_seed", &self.messages)?;
        // The fork records into its own session directory and goes on in the
        // same chat, which holds the history it carries.
        self.leave_memory()?;
        self.recorder = recorder;
        self.reconcile_completion = true;
        self.session_context_pending = Some("fork");
        Ok(self.recorder.path())
    }

    /// Branch the exact live model window into a child ledger.
    pub(crate) fn branch(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        let parent_entry = self.recorder.active_leaf()?;
        let mut recorder = prepare_child(cwd, &parent, parent_entry, "branch")?;
        recorder.record_context("branch_seed", &self.messages)?;
        // As for a fork: the same chat, from the branch's next step.
        self.leave_memory()?;
        self.recorder = recorder;
        self.reconcile_completion = true;
        self.session_context_pending = Some("branch");
        Ok(self.recorder.path())
    }
}
