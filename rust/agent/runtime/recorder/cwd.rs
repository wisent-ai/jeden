//! Moving a recorded session to another working directory (`/move`): the
//! session's `state.json` is rewritten through the shared durable replace,
//! keeping the state file's permissions.
use super::*;

impl SessionRecorder {
    pub(in crate::agent) fn set_cwd(&mut self, cwd: &Path) -> Result<(), String> {
        self.ensure().map_err(|error| {
            format!(
                "cannot prepare session {} for working-directory change: {error}",
                self.dir.display()
            )
        })?;
        let state_path = self.dir.join("state.json");
        let metadata = fs::symlink_metadata(&state_path).map_err(|error| {
            format!(
                "cannot inspect session state {}: {error}",
                state_path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "cannot change working directory: {} is not a regular state file",
                state_path.display()
            ));
        }
        let text = fs::read_to_string(&state_path).map_err(|error| {
            format!(
                "cannot read session state {}: {error}",
                state_path.display()
            )
        })?;
        let mut state: Value = serde_json::from_str(&text)
            .map_err(|e| format!("invalid {}: {}", state_path.display(), e))?;
        let object = state
            .as_object_mut()
            .ok_or_else(|| format!("invalid {}: expected object", state_path.display()))?;
        object.insert("cwd".into(), json!(cwd));
        let encoded = serde_json::to_vec_pretty(&state).map_err(|error| {
            format!(
                "cannot encode session state {}: {error}",
                state_path.display()
            )
        })?;
        crate::tool_runtime::runtime_ops::durable::replace_keeping_mode(&state_path, &encoded)
            .map_err(|cause| format!("cannot publish session state: {cause}"))?;
        self.cwd = cwd.to_path_buf();
        Ok(())
    }
}
