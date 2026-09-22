use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const DATASET_SCHEMA: &str = "jeden.eval-dataset.v1";
pub const FIXTURE_SCHEMA: &str = "jeden.eval-fixture.v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalDatasetV1 {
    pub schema: String,
    pub id: String,
    pub version: String,
    pub license: String,
    pub provenance: String,
    pub cases: Vec<EvalCaseV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalCaseV1 {
    pub id: String,
    pub fixture: String,
    pub prompt: String,
    #[serde(default)]
    pub allowed_capabilities: BTreeSet<String>,
    #[serde(default)]
    pub required_capabilities: BTreeSet<String>,
    pub budget: EvalBudgetV1,
    pub graders: Vec<GraderSpecV1>,
    #[serde(default)]
    pub expected_artifacts: Vec<ExpectedArtifactV1>,
    #[serde(default)]
    pub forbidden_actions: BTreeSet<String>,
    pub tags: BTreeSet<String>,
    pub seed: u64,
    pub provenance: String,
    pub license: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvalBudgetV1 {
    pub max_steps: u32,
    pub max_tool_calls: u32,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_microunits: u64,
    pub max_elapsed_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedArtifactV1 {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum GraderSpecV1 {
    FileEquals {
        id: String,
        path: String,
        content: String,
        points: u32,
        hard: bool,
    },
    FileContains {
        id: String,
        path: String,
        needle: String,
        points: u32,
        hard: bool,
    },
    FileAbsent {
        id: String,
        path: String,
        points: u32,
        hard: bool,
    },
    JsonEquals {
        id: String,
        path: String,
        expected: serde_json::Value,
        points: u32,
        hard: bool,
    },
    JsonSchema {
        id: String,
        path: String,
        schema: serde_json::Value,
        points: u32,
        hard: bool,
    },
    Process {
        id: String,
        argv: Vec<String>,
        expected_exit: i32,
        stdout_contains: Option<String>,
        points: u32,
        hard: bool,
    },
    ArtifactSha256 {
        id: String,
        path: String,
        sha256: String,
        points: u32,
        hard: bool,
    },
}

impl GraderSpecV1 {
    pub fn id(&self) -> &str {
        match self {
            Self::FileEquals { id, .. }
            | Self::FileContains { id, .. }
            | Self::FileAbsent { id, .. }
            | Self::JsonEquals { id, .. }
            | Self::JsonSchema { id, .. }
            | Self::Process { id, .. }
            | Self::ArtifactSha256 { id, .. } => id,
        }
    }
    pub fn points(&self) -> u32 {
        match self {
            Self::FileEquals { points, .. }
            | Self::FileContains { points, .. }
            | Self::FileAbsent { points, .. }
            | Self::JsonEquals { points, .. }
            | Self::JsonSchema { points, .. }
            | Self::Process { points, .. }
            | Self::ArtifactSha256 { points, .. } => *points,
        }
    }
    pub fn hard(&self) -> bool {
        match self {
            Self::FileEquals { hard, .. }
            | Self::FileContains { hard, .. }
            | Self::FileAbsent { hard, .. }
            | Self::JsonEquals { hard, .. }
            | Self::JsonSchema { hard, .. }
            | Self::Process { hard, .. }
            | Self::ArtifactSha256 { hard, .. } => *hard,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixtureV1 {
    pub schema: String,
    pub license: String,
    pub provenance: String,
    pub files: BTreeMap<String, String>,
}

pub fn safe_relative(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("path must be a normalized relative path: {value}"));
    }
    Ok(path.to_path_buf())
}

pub fn load_dataset(path: &Path) -> Result<EvalDatasetV1, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read dataset {}: {error}", path.display()))?;
    let dataset: EvalDatasetV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid dataset {}: {error}", path.display()))?;
    validate_dataset(&dataset)?;
    Ok(dataset)
}

mod validate;

pub use validate::{load_fixture, validate_dataset, validate_sha256};
