use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskLimits {
    pub max_parallel: usize,
    pub max_batch: usize,
    pub max_depth: u32,
    pub max_children: usize,
    pub max_output_bytes: u64,
    pub wait_timeout_ms: u64,
    pub kill_grace_ms: u64,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self { max_parallel: 4, max_batch: 32, max_depth: 4, max_children: 16, max_output_bytes: 2 * 1024 * 1024, wait_timeout_ms: 300_000, kill_grace_ms: 1_500 }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnPolicy {
    #[serde(default)]
    pub allow_agents: Vec<String>,
    #[serde(default)]
    pub deny_agents: Vec<String>,
    #[serde(default)]
    pub allow_recursive: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub spawn: SpawnPolicy,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(skip)]
    pub source: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus { Queued, Running, Waiting, Succeeded, Failed, Cancelled, Interrupted }

impl JobStatus {
    pub fn terminal(&self) -> bool { matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted) }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub task: String,
    pub agent: String,
    pub status: JobStatus,
    pub cwd: PathBuf,
    pub workspace: PathBuf,
    pub isolation: String,
    pub session_path: PathBuf,
    pub stdout: PathBuf,
    pub stderr: PathBuf,
    pub capture: PathBuf,
    pub pid: Option<u32>,
    pub parent_job: Option<String>,
    pub depth: u32,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub delivered: bool,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub body: String,
    pub correlation_id: Option<String>,
    pub reply_to: Option<String>,
    pub created_at: u64,
    pub delivered_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityHealth {
    pub id: &'static str,
    pub healthy: bool,
    pub store: PathBuf,
    pub discovered_agents: usize,
    pub running: usize,
    pub limits: TaskLimits,
    pub isolation_strategies: Vec<&'static str>,
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum TaskError {
    Io(String), Invalid(String), NotFound(String), Capacity { running: usize, limit: usize },
    RecursionDenied { agent: String, depth: u32 }, Cancelled(String), Timeout(String), Conflict(String), Process(String),
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(v) | Self::Invalid(v) | Self::NotFound(v) | Self::Cancelled(v) | Self::Timeout(v) | Self::Conflict(v) | Self::Process(v) => f.write_str(v),
            Self::Capacity { running, limit } => write!(f, "task capacity exhausted: {running}/{limit} jobs running"),
            Self::RecursionDenied { agent, depth } => write!(f, "spawn policy denied recursive agent {agent} at depth {depth}"),
        }
    }
}
impl std::error::Error for TaskError {}
impl From<std::io::Error> for TaskError { fn from(value: std::io::Error) -> Self { Self::Io(value.to_string()) } }
impl From<serde_json::Error> for TaskError { fn from(value: serde_json::Error) -> Self { Self::Invalid(value.to_string()) } }
