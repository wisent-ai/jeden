use super::*;

impl AgentSession {
    pub fn session_path(&self) -> Result<PathBuf, String> {
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        // The path is replaced whole, so a writer that panicked left a path.
        Ok(crate::panicked::read_held(&self.inner.session_path).clone())
    }

    pub fn completion(&self) -> Result<serde_json::Value, String> {
        crate::completion::snapshot(&self.session_path()?)
    }

    pub fn add_request(&self, prompt: &str) -> Result<serde_json::Value, String> {
        let path = self.session_path()?;
        let cwd = crate::completion::cli::workspace(&path)?;
        let (_, state) = crate::completion::capture_request(&path, &cwd, prompt)?;
        let value = crate::completion::snapshot_value(&state);
        let at = crate::agent::now_stamp()?;
        crate::cli::sessions::append_ledger_entry(&path, at, "completion_state", value.clone())?;
        Ok(value)
    }

    pub fn control_completion(
        &self,
        task_id: &str,
        action: &str,
        reason: &str,
        revision: u64,
    ) -> Result<serde_json::Value, String> {
        let path = self.session_path()?;
        let action = crate::completion::ControlAction::named(action)?;
        let state = crate::completion::operator_control(&path, task_id, action, reason, revision)?;
        // Active work stops when the task the operator controlled is left
        // paused or cancelled, read from the state the control produced
        // rather than from the action's spelling.
        let stopped = state.tasks.iter().any(|task| {
            task.id == task_id
                && matches!(
                    task.status,
                    crate::completion::TaskStatus::Paused
                        | crate::completion::TaskStatus::Cancelled
                )
        });
        if stopped {
            for cancel in crate::panicked::held(&self.inner.active).values() {
                cancel.store(true, Ordering::Release);
            }
            crate::tool_runtime::runtime_ops::announce_cancellation();
        }
        let value = crate::completion::snapshot_value(&state);
        let at = crate::agent::now_stamp()?;
        crate::cli::sessions::append_ledger_entry(&path, at, "completion_state", value.clone())?;
        Ok(value)
    }
}
