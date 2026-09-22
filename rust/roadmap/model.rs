//! What a roadmap item is, and every way a roadmap operation can fail.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapStatus {
    Backlog,
    Planned,
    InProgress,
    Implemented,
    NotRun,
    Failed,
    ExternalBlocked,
    Passed,
    Dropped,
}

impl RoadmapStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Planned => "planned",
            Self::InProgress => "in_progress",
            Self::Implemented => "implemented",
            Self::NotRun => "not_run",
            Self::Failed => "failed",
            Self::ExternalBlocked => "external_blocked",
            Self::Passed => "passed",
            Self::Dropped => "dropped",
        }
    }
}

impl fmt::Display for RoadmapStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RoadmapStatus {
    type Err = RoadmapError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "backlog" => Ok(Self::Backlog),
            "planned" => Ok(Self::Planned),
            "in_progress" => Ok(Self::InProgress),
            "implemented" => Ok(Self::Implemented),
            "not_run" => Ok(Self::NotRun),
            "failed" => Ok(Self::Failed),
            "external_blocked" => Ok(Self::ExternalBlocked),
            "passed" => Ok(Self::Passed),
            "dropped" => Ok(Self::Dropped),
            other => Err(RoadmapError::Invalid(format!(
                "unknown roadmap status '{other}'; expected backlog|planned|in_progress|implemented|not_run|failed|external_blocked|passed|dropped"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceLink {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_id: Option<String>,
    pub added_at: String,
    pub added_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapItem {
    pub id: String,
    pub title: String,
    pub area: String,
    pub priority: String,
    pub status: RoadmapStatus,
    pub summary: String,
    #[serde(default)]
    pub implementation: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub implementation_order: String,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub external_prerequisites: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_at: String,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapFile {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub items: Vec<RoadmapItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraph {
    pub revision: u64,
    pub nodes: Vec<RoadmapGraphNode>,
    pub edges: Vec<RoadmapGraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraphNode {
    pub id: String,
    pub title: String,
    pub status: RoadmapStatus,
    pub priority: String,
    pub area: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub ok: bool,
    pub schema_version: u32,
    pub revision: u64,
    pub item_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum RoadmapError {
    Io(String),
    Invalid(String),
    RevisionConflict { expected: u64, actual: u64 },
    NotFound(String),
    Usage(String),
}

impl fmt::Display for RoadmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Invalid(message) | Self::Usage(message) => {
                f.write_str(message)
            }
            Self::RevisionConflict { expected, actual } => write!(
                f,
                "roadmap revision conflict: expected {expected}, found {actual}"
            ),
            Self::NotFound(id) => write!(f, "roadmap item not found: {id}"),
        }
    }
}

impl From<std::io::Error> for RoadmapError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<String> for RoadmapError {
    fn from(error: String) -> Self {
        Self::Io(error)
    }
}
