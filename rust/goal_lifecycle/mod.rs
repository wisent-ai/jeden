//! Background goal-lifecycle classification via Oko's local qualified model.
//!
//! Each classified user prompt is sent to the loopback OpenAI-compatible
//! endpoint served by `com.wisent.compute.service.oko-goal-lifecycle`
//! (`mlx_lm.server`). Everything here is fail-open: when the service is
//! unreachable the first probe caches the verdict for the process lifetime and
//! every later call is a fast no-op, so a Jeden turn never blocks on Oko.

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Model label recorded in ledger events; also the served-id fallback when
/// `GET {base}/v1/models` does not name the loaded model.
pub const LIFECYCLE_MODEL_LABEL: &str = "oko-goal-lifecycle-v1";

mod classify;
mod title;

pub use classify::classify;
pub use title::resolve_goal_title;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    StartGoal,
    ContinueCurrent,
    FinishGoal,
    Ignore,
}

impl LifecycleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StartGoal => "startGoal",
            Self::ContinueCurrent => "continueCurrent",
            Self::FinishGoal => "finishGoal",
            Self::Ignore => "ignore",
        }
    }
}

/// Strictly parsed model verdict. Unknown `action` or `lifecycle_evidence`
/// values reject the whole reply (treated as "service said nothing").
#[derive(Debug, Clone)]
pub struct LifecycleDecision {
    pub action: LifecycleAction,
    pub goal_ref: String,
    pub lifecycle_evidence: String,
}

pub struct LifecycleRequest {
    pub prompt: String,
    pub session_id: String,
    pub turn_index: u64,
    /// Current active goal objective, when goal mode holds one.
    pub goal_objective: Option<String>,
}


/// A `(text, status)` sink for goal-lifecycle events, shared rather than
/// borrowed because the background threads below outlive the turn that
/// started them.
pub(crate) type GoalEventSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Background classification for one user turn. Never blocks the caller:
/// spawns a thread that classifies the prompt, records the `goal_lifecycle`
/// ledger event, emits the RPC `goal` session event when a sink exists, and —
/// only when `/goal auto on` — actuates goal mode state. Returns the thread's
/// handle so the end-of-turn completion judge can order itself after this
/// classification; a fast turn would otherwise let the classifier's
/// `startGoal` land after the judge and re-open a goal the judge just closed.
pub(crate) fn spawn_turn_classification(
    cwd: PathBuf,
    prompt: String,
    session_dir: PathBuf,
    turn_index: u64,
    goal_event: Option<GoalEventSink>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let state = crate::slash::read_mode_state(&cwd);
        let goal_objective = Some(state.goal.objective.trim())
            .filter(|objective| state.goal.enabled && !objective.is_empty())
            .map(str::to_string);
        let session_id = session_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());
        let Some(mut decision) = classify(&LifecycleRequest {
            prompt: prompt.clone(),
            session_id,
            turn_index,
            goal_objective: goal_objective.clone(),
        }) else {
            return;
        };
        // Prompt classification can suggest a title, but cannot independently
        // close retained work or outrank the native acceptance decision.
        if decision.action == LifecycleAction::FinishGoal
            && !crate::completion::read_state(&session_dir).is_ok_and(|state| state.complete())
        {
            decision.action = LifecycleAction::ContinueCurrent;
        }
        let resolved_goal = match decision.action {
            LifecycleAction::StartGoal => Some(resolve_goal_title(&prompt)),
            LifecycleAction::ContinueCurrent | LifecycleAction::FinishGoal => {
                goal_objective.clone()
            }
            LifecycleAction::Ignore => None,
        };
        let _ = crate::cli::sessions::append_ledger_entry(
            &session_dir,
            crate::agent::now_stamp(),
            "goal_lifecycle",
            json!({
                "action": decision.action.as_str(),
                "goal_ref": decision.goal_ref,
                "lifecycle_evidence": decision.lifecycle_evidence,
                "goal": resolved_goal,
                "model": LIFECYCLE_MODEL_LABEL,
            }),
        );
        match decision.action {
            LifecycleAction::StartGoal => {
                let title = resolved_goal.expect("startGoal always resolves a title");
                if state.goal.auto {
                    let objective = title.clone();
                    let _ = crate::slash::mutate_mode_state(&cwd, move |state| {
                        state.goal.objective = objective;
                        state.goal.enabled = true;
                        state.goal.paused = false;
                        Ok(())
                    });
                }
                if let Some(emit) = &goal_event {
                    emit(&title, "active");
                }
            }
            LifecycleAction::FinishGoal => {
                if state.goal.auto {
                    // Mirror `/goal drop`.
                    let _ = crate::slash::mutate_mode_state(&cwd, |state| {
                        state.goal.enabled = false;
                        state.goal.paused = false;
                        state.goal.objective.clear();
                        state.goal.budget = None;
                        Ok(())
                    });
                }
                if let (Some(emit), Some(objective)) = (&goal_event, &goal_objective) {
                    emit(objective, "done");
                }
            }
            LifecycleAction::ContinueCurrent | LifecycleAction::Ignore => {}
        }
    })
}

/// Goal completion follows the native acceptance result, never a post-answer
/// interpretation of the execution agent's own claims.
pub(crate) fn finish_verified_goal(
    cwd: &Path,
    session_dir: &Path,
    goal_event: Option<&GoalEventSink>,
    classification: Option<std::thread::JoinHandle<()>>,
) -> Result<(), String> {
    if let Some(handle) = classification {
        let _ = handle.join();
    }
    let completion = crate::completion::read_state(session_dir)?;
    if !completion.complete() {
        return Err("cannot finish goal while retained work remains unverified".into());
    }
    let state = crate::slash::read_mode_state(cwd);
    let objective = state.goal.objective.trim();
    if !state.goal.enabled || objective.is_empty() {
        return Ok(());
    }
    crate::cli::sessions::append_ledger_entry(
        session_dir,
        crate::agent::now_stamp(),
        "goal_lifecycle",
        json!({"action": "finishGoal", "judge": "verified_tasks",
            "goal": objective, "completionRevision": completion.revision}),
    )?;
    if state.goal.auto {
        crate::slash::mutate_mode_state(cwd, |state| {
            state.goal.enabled = false;
            state.goal.paused = false;
            state.goal.objective.clear();
            state.goal.budget = None;
            Ok(())
        })?;
    }
    if let Some(emit) = goal_event {
        emit(objective, "done");
    }
    Ok(())
}
