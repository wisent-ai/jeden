//! Background goal-lifecycle classification by the lifecycle model, asked
//! through Brama under the alias this machine declares in
//! `JEDEN_LIFECYCLE_MODEL_ALIAS`, with the bearer and signature every other
//! Jeden model call carries. A machine that declares no alias has no
//! classifier; a refused call is written to the session ledger as
//! `goal_lifecycle_refused` with Brama's own reason, and the turn proceeds.

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model_router::ChatConfig;

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
    router: ChatConfig,
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
        // A session directory without a final name identifies no session;
        // the classification is refused and recorded rather than filed
        // under an invented session id.
        let Some(session_id) = session_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            recorded(
                &session_dir,
                "goal_lifecycle_refused",
                json!({
                    "alias": classify::alias(),
                    "reason": format!(
                        "session directory {} has no final name to identify the session",
                        session_dir.display()
                    ),
                }),
            );
            return;
        };
        let classified = classify(
            &router,
            &LifecycleRequest {
                prompt: prompt.clone(),
                session_id,
                turn_index,
                goal_objective: goal_objective.clone(),
            },
        );
        let mut decision = match classified {
            Ok(Some(decision)) => decision,
            Ok(None) => return,
            Err(reason) => {
                recorded(
                    &session_dir,
                    "goal_lifecycle_refused",
                    json!({ "alias": classify::alias(), "reason": reason }),
                );
                return;
            }
        };
        // Prompt classification can suggest a title, but cannot independently
        // close retained work or outrank the native acceptance decision.
        if decision.action == LifecycleAction::FinishGoal
            && !crate::completion::read_state(&session_dir).is_ok_and(|state| state.complete())
        {
            decision.action = LifecycleAction::ContinueCurrent;
        }
        let resolved_goal = match decision.action {
            LifecycleAction::StartGoal => match resolve_goal_title(&prompt) {
                Ok(title) => Some(title),
                Err(reason) => {
                    recorded(
                        &session_dir,
                        "goal_lifecycle_refused",
                        json!({ "alias": classify::alias(), "reason": reason }),
                    );
                    return;
                }
            },
            LifecycleAction::ContinueCurrent | LifecycleAction::FinishGoal => {
                goal_objective.clone()
            }
            LifecycleAction::Ignore => None,
        };
        recorded(
            &session_dir,
            "goal_lifecycle",
            json!({
                "action": decision.action.as_str(),
                "goal_ref": decision.goal_ref,
                "lifecycle_evidence": decision.lifecycle_evidence,
                "goal": resolved_goal,
                "model": classify::alias(),
            }),
        );
        // A goal state that could not be saved is said and not announced:
        // the event would show a goal the session does not hold.
        match decision.action {
            LifecycleAction::StartGoal => {
                let title = resolved_goal.expect("startGoal always resolves a title");
                if state.goal.auto {
                    let objective = title.clone();
                    let saved = crate::slash::mutate_mode_state(&cwd, move |state| {
                        state.goal.objective = objective;
                        state.goal.enabled = true;
                        state.goal.paused = false;
                        Ok(())
                    });
                    if let Err(cause) = saved {
                        eprintln!("jeden: the goal \"{title}\" is not started: {cause}");
                        return;
                    }
                }
                if let Some(emit) = &goal_event {
                    emit(&title, "active");
                }
            }
            LifecycleAction::FinishGoal => {
                if state.goal.auto {
                    // Mirror `/goal drop`.
                    let saved = crate::slash::mutate_mode_state(&cwd, |state| {
                        state.goal.enabled = false;
                        state.goal.paused = false;
                        state.goal.objective.clear();
                        state.goal.budget = None;
                        Ok(())
                    });
                    if let Err(cause) = saved {
                        eprintln!("jeden: the finished goal is not dropped: {cause}");
                        return;
                    }
                }
                if let (Some(emit), Some(objective)) = (&goal_event, &goal_objective) {
                    emit(objective, "done");
                }
            }
            LifecycleAction::ContinueCurrent | LifecycleAction::Ignore => {}
        }
    })
}

/// Append one lifecycle entry to the session ledger; an entry that cannot be
/// written is said, since the ledger is where the decision is read back.
fn recorded(session_dir: &Path, kind: &str, data: serde_json::Value) {
    if let Err(cause) = crate::cli::sessions::append_ledger_entry(
        session_dir,
        crate::agent::now_stamp(),
        kind,
        data,
    ) {
        eprintln!(
            "jeden: the {kind} entry is not in {}: {cause}",
            session_dir.display()
        );
    }
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
        if handle.join().is_err() {
            eprintln!(
                "jeden: the turn's goal classification panicked; its decision is not recorded"
            );
        }
    }
    let completion = crate::completion::read_state(session_dir)?;
    if !completion.complete() {
        return Err("cannot finish goal while retained work remains unverified".into());
    }
    let state = crate::slash::state::try_read_mode_state(cwd)?;
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
