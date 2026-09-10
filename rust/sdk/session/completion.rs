use super::*;

impl AgentSession {
    pub fn session_path(&self) -> Result<PathBuf, String> {
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        self.inner.session_path.read().map(|path| path.clone())
            .map_err(|_| "session path lock poisoned".into())
    }

    pub fn completion(&self) -> Result<serde_json::Value, String> {
        crate::completion::snapshot(&self.session_path()?)
    }

    pub fn add_request(&self, prompt: &str) -> Result<serde_json::Value, String> {
        let path = self.session_path()?;
        let cwd = crate::completion::cli::workspace(&path)?;
        let (_, state) = crate::completion::capture_request(&path, &cwd, prompt)?;
        let value = crate::completion::snapshot_value(&state);
        crate::cli::sessions::append_ledger_entry(&path, crate::agent::now_stamp(), "completion_state", value.clone())?;
        Ok(value)
    }

    pub fn control_completion(&self, task_id: &str, action: &str, reason: &str, revision: u64) -> Result<serde_json::Value, String> {
        let path = self.session_path()?;
        let state = crate::completion::operator_control(&path, task_id, action, reason, revision)?;
        if matches!(action, "pause" | "cancel") {
            for cancel in self.inner.active.lock().map_err(|_| "active request lock poisoned")?.values() {
                cancel.store(true, Ordering::Release);
            }
        }
        let value = crate::completion::snapshot_value(&state);
        crate::cli::sessions::append_ledger_entry(&path, crate::agent::now_stamp(), "completion_state", value.clone())?;
        Ok(value)
    }
}
