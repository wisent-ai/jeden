//! Moving a recorded session to another working directory (`/move`): the
//! session's `state.json` is rewritten through a staged file.
use super::*;
use std::io::Write;

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
        let staging = self
            .dir
            .join(format!(".state-{}.new", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| {
                format!(
                    "cannot create session state staging file {}: {error}",
                    staging.display()
                )
            })?;
        let publish = (|| {
            file.set_permissions(metadata.permissions())
                .map_err(|error| {
                    format!(
                        "cannot preserve session state permissions {}: {error}",
                        staging.display()
                    )
                })?;
            file.write_all(&encoded).map_err(|error| {
                format!(
                    "cannot write session state staging file {}: {error}",
                    staging.display()
                )
            })?;
            file.sync_all().map_err(|error| {
                format!(
                    "cannot sync session state staging file {}: {error}",
                    staging.display()
                )
            })?;
            fs::rename(&staging, &state_path).map_err(|error| {
                format!(
                    "cannot publish session state {} as {}: {error}",
                    staging.display(),
                    state_path.display()
                )
            })
        })();
        if let Err(error) = publish {
            return match fs::remove_file(&staging) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!(
                    "{error}; cannot remove staging file {}: {cleanup}",
                    staging.display()
                )),
            };
        }
        self.cwd = cwd.to_path_buf();
        Ok(())
    }
}
