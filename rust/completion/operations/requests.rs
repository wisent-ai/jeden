use super::super::{
    model::{CompletionState, WorkRequest},
    store,
};
use std::path::Path;

pub(crate) fn capture_request(
    session: &Path,
    cwd: &Path,
    prompt: &str,
) -> Result<(String, CompletionState), String> {
    if prompt.trim().is_empty() {
        return Err("completion request must not be empty".into());
    }
    store::update(session, None, |state| {
        let id = uuid::Uuid::new_v4().to_string();
        state.requests.push(WorkRequest {
            id: id.clone(),
            prompt: prompt.to_string(),
            cwd: cwd.display().to_string(),
            paused: false,
            captured_at: crate::agent::now_stamp(),
            planned: false,
            coverage_verified: false,
            estimate: None,
            completed_at: None,
        });
        state.blocker = None;
        Ok(id)
    })
}
