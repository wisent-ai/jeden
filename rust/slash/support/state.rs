use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ModeState {
    #[serde(default)]
    pub(crate) plan: PlanState,
    #[serde(default)]
    pub(crate) goal: GoalState,
    #[serde(rename = "guidedGoal", default)]
    pub(crate) guided_goal: GuidedGoalState,
    #[serde(default)]
    pub(crate) loop_mode: LoopState,
    #[serde(default)]
    pub(crate) fast: FastState,
    #[serde(default)]
    pub(crate) advisor: AdvisorState,
    #[serde(default)]
    pub(crate) force: Option<ForceState>,
    #[serde(rename = "lastFailedTask", default)]
    pub(crate) last_failed_task: String,
    #[serde(rename = "lastTask", default)]
    pub(crate) last_task: String,
    #[serde(default)]
    pub(crate) compact: bool,
    #[serde(default)]
    pub(crate) shake: String,
    /// Migration input only; live task state belongs to the session completion store.
    #[serde(rename = "todos", default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) legacy_todos: Vec<TodoState>,
    #[serde(default)]
    pub(crate) branches: Vec<BranchState>,
    #[serde(rename = "activeRoadmapItem", default)]
    pub(crate) active_roadmap_item: Option<String>,
    #[serde(rename = "lastSessionPath", default)]
    pub(crate) last_session_path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) tools: ToolsState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ToolsState {
    #[serde(rename = "approvalMode", default)]
    pub(crate) approval_mode: String,
    #[serde(default)]
    pub(crate) approval: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PlanState {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(rename = "latestPlan", default)]
    pub(crate) latest_plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GuidedGoalState {
    #[serde(default)]
    pub(crate) active: bool,
    #[serde(rename = "roughObjective", default)]
    pub(crate) rough_objective: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GoalState {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) paused: bool,
    #[serde(default)]
    pub(crate) objective: String,
    #[serde(default)]
    pub(crate) budget: Option<f64>,
    /// When true, Oko's goal-lifecycle model may start and finish goals
    /// automatically from classified user prompts.
    #[serde(default)]
    pub(crate) auto: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct LoopState {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) remaining: Option<u64>,
    #[serde(default)]
    pub(crate) until: Option<u64>,
    #[serde(default)]
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FastState {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(rename = "serviceTier", default = "default_service_tier")]
    pub(crate) service_tier: String,
}

impl Default for FastState {
    fn default() -> Self {
        Self {
            enabled: false,
            service_tier: default_service_tier(),
        }
    }
}

pub(crate) fn default_service_tier() -> String {
    "priority".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AdvisorState {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) model: String,
    #[serde(rename = "lastReview", default)]
    pub(crate) last_review: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ForceState {
    #[serde(default)]
    pub(crate) tool: String,
    #[serde(default)]
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct TodoState {
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(rename = "createdAt", default)]
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct BranchState {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(rename = "createdAt", default)]
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) path: String,
    #[serde(rename = "roadmapItem", default)]
    pub(crate) roadmap_item: Option<String>,
}

pub(crate) fn mode_state_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/mode-state.json")
}

/// Create the directory that holds mode state and keep that state out of
/// version control. Mode state names the working directory, which includes the
/// operator's home path, so a checkout that commits it publishes that path.
/// The ignore file is written once, beside the state, so it holds in every
/// checkout Jeden works in without each repository listing it.
pub(crate) fn ensure_mode_state_dir(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(".gitignore"))
    {
        Ok(mut file) => file
            .write_all(b"mode-state.json\n.mode-state*\n")
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

// The kernel's advisory lock on the lock file: a second writer blocks until the
// first one's file closes, which also happens when its process dies, so there
// is no retry count, no pause between tries and no stale lock file.
struct ModeStateLock {
    _file: File,
}

impl ModeStateLock {
    fn acquire(cwd: &Path) -> Result<Self, String> {
        let path = cwd.join(".jeden/.mode-state.lock");
        if let Some(parent) = path.parent() {
            ensure_mode_state_dir(parent)?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("mode-state lock {} cannot open: {error}", path.display()))?;
        file.lock()
            .map_err(|error| format!("mode-state lock {} refused: {error}", path.display()))?;
        Ok(Self { _file: file })
    }
}

pub(crate) fn mutate_mode_state(
    cwd: &Path,
    change: impl FnOnce(&mut ModeState) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = ModeStateLock::acquire(cwd)?;
    let mut state = try_read_mode_state(cwd)?;
    change(&mut state)?;
    write_mode_state(cwd, &state)
}

/// The saved mode state; the default state when none was saved yet. A file
/// that cannot be read, does not parse, or holds a value of the wrong type is
/// refused naming the file: decoded as the default, one bad field turned off
/// every mode and forgot the goal and the last session, and the next change
/// wrote those defaults over the file.
pub(crate) fn try_read_mode_state(cwd: &Path) -> Result<ModeState, String> {
    let path = mode_state_path(cwd);
    serde_json::from_value(read_mode_state_value(cwd)?)
        .map_err(|error| format!("{} is not a valid mode state: {error}", path.display()))
}

/// The saved mode state for a reader that has no error to return. `jeden`
/// refuses to start while the mode state is unreadable or invalid
/// (`check_mode_state`), so a failure here means the file broke during the
/// run; the run stops with the cause instead of continuing, or saving, with
/// every mode off.
pub(crate) fn read_mode_state(cwd: &Path) -> ModeState {
    match try_read_mode_state(cwd) {
        Ok(state) => state,
        Err(error) => panic!("{error}"),
    }
}

/// Refuse to start on a mode state that cannot be read, naming the file.
pub(crate) fn check_mode_state(cwd: &Path) -> Result<(), String> {
    try_read_mode_state(cwd).map(drop)
}

/// The saved mode state as JSON, for the readers that keep fields `ModeState`
/// does not declare; an empty object when none was saved yet. Refused like
/// `read_mode_state`, and when the root is not an object.
pub(crate) fn read_mode_state_value(cwd: &Path) -> Result<Value, String> {
    let path = mode_state_path(cwd);
    let state = crate::slash::common::read_json_value(&path)?;
    if !state.is_object() {
        return Err(format!(
            "{} does not hold a JSON object, so the mode state is unknown",
            path.display()
        ));
    }
    Ok(state)
}

pub(crate) fn write_mode_state(cwd: &Path, state: &ModeState) -> Result<(), String> {
    let path = mode_state_path(cwd);
    let parent = path
        .parent()
        .ok_or_else(|| "mode-state path has no parent".to_string())?;
    ensure_mode_state_dir(parent)?;
    let text = serde_json::to_string_pretty(state).map_err(|error| error.to_string())? + "\n";
    crate::tool_runtime::runtime_ops::durable::replace(&path, text.as_bytes())
}
