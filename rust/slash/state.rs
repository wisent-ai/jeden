use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
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
    #[serde(default)]
    pub(crate) todos: Vec<TodoState>,
    #[serde(default)]
    pub(crate) branches: Vec<BranchState>,
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
    fn default() -> Self { Self { enabled: false, service_tier: default_service_tier() } }
}

pub(crate) fn default_service_tier() -> String { "priority".to_string() }

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
}

pub(crate) fn mode_state_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/mode-state.json") }

pub(crate) fn read_mode_state(cwd: &Path) -> ModeState {
    fs::read_to_string(mode_state_path(cwd))
        .ok()
        .and_then(|text| serde_json::from_str::<ModeState>(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn write_mode_state(cwd: &Path, state: &ModeState) -> Result<(), String> {
    let path = mode_state_path(cwd);
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}
