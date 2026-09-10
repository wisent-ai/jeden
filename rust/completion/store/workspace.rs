use super::{update, CompletionState, WorkRequest};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Old workspace todo fields are migration input, never another task authority.
/// The workspace lock is held until the session copy is durable and the old
/// fields have been cleared. A repeated migration recognizes the same request.
pub(crate) fn migrate(cwd: &Path, destination: Option<&Path>) -> Result<(), String> {
    if crate::slash::read_mode_state(cwd).legacy_todos.is_empty() {
        return Ok(());
    }
    crate::slash::mutate_mode_state(cwd, |mode| {
        if mode.legacy_todos.is_empty() {
            return Ok(());
        }
        let session = match destination
            .map(Path::to_path_buf)
            .or_else(|| mode.last_session_path.clone())
        {
            Some(path) => path,
            None => crate::agent::Conversation::new(cwd)?.session_path(),
        };
        let cwd_text = cwd.display().to_string();
        let id = format!("legacy-workspace-{:x}", Sha256::digest(cwd_text.as_bytes()));
        update(&session, None, |state: &mut CompletionState| {
            if state.requests.iter().any(|request| request.id == id) {
                return Ok(());
            }
            let items = mode
                .legacy_todos
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            state.requests.push(WorkRequest {
                id: id.clone(),
                prompt: format!("Inspect prior results and finish every retained workspace task below. Old status labels were claims, not verification; do not repeat effects already established by evidence.\n{items}"),
                cwd: cwd_text.clone(),
                paused: false,
                captured_at: crate::agent::now_stamp(),
                planned: false,
                coverage_verified: false,
            });
            Ok(())
        })?;
        mode.legacy_todos.clear();
        mode.last_session_path = Some(session);
        Ok(())
    })
}
