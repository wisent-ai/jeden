use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

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

struct ModeStateLock {
    path: PathBuf,
    _file: File,
}

impl Drop for ModeStateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl ModeStateLock {
    fn acquire(cwd: &Path) -> Result<Self, String> {
        let path = cwd.join(".jeden/.mode-state.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        for _ in 0..500 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).map_err(|error| error.to_string())?;
                    file.sync_all().map_err(|error| error.to_string())?;
                    return Ok(Self { path, _file: file });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err(format!(
            "timed out waiting for mode-state lock {}",
            path.display()
        ))
    }
}

pub(crate) fn mutate_mode_state(
    cwd: &Path,
    change: impl FnOnce(&mut ModeState) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = ModeStateLock::acquire(cwd)?;
    let mut state = read_mode_state(cwd);
    change(&mut state)?;
    write_mode_state(cwd, &state)
}

pub(crate) fn read_mode_state(cwd: &Path) -> ModeState {
    fs::read_to_string(mode_state_path(cwd))
        .ok()
        .and_then(|text| serde_json::from_str::<ModeState>(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn write_mode_state(cwd: &Path, state: &ModeState) -> Result<(), String> {
    let path = mode_state_path(cwd);
    let parent = path
        .parent()
        .ok_or_else(|| "mode-state path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(
        ".mode-state.json.tmp-{}-{nonce}",
        std::process::id()
    ));
    let text = serde_json::to_string_pretty(state).map_err(|error| error.to_string())? + "\n";
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        file.write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temp, &path).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
