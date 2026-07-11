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

pub fn validate_dataset(dataset: &EvalDatasetV1) -> Result<(), String> {
    if dataset.schema != DATASET_SCHEMA {
        return Err(format!("unsupported dataset schema {}", dataset.schema));
    }
    if dataset.id.trim().is_empty() || dataset.version.trim().is_empty() {
        return Err("dataset id and version are required".into());
    }
    if dataset.license.trim().is_empty() || dataset.provenance.trim().is_empty() {
        return Err("dataset license and provenance are required".into());
    }
    validate_license(&dataset.license)?;
    reject_sensitive_text("dataset provenance", &dataset.provenance)?;
    if !(20..=50).contains(&dataset.cases.len()) {
        return Err(format!(
            "reference dataset must contain 20..=50 cases, got {}",
            dataset.cases.len()
        ));
    }
    let mut ids = BTreeSet::new();
    let mut prompts = BTreeSet::new();
    for case in &dataset.cases {
        if !ids.insert(case.id.as_str()) {
            return Err(format!("duplicate eval case id {}", case.id));
        }
        let normalized_prompt = case
            .prompt
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if !prompts.insert(normalized_prompt) {
            return Err(format!("duplicate eval prompt in case {}", case.id));
        }
        safe_relative(&case.fixture)?;
        if case.prompt.trim().is_empty()
            || case.license.trim().is_empty()
            || case.provenance.trim().is_empty()
        {
            return Err(format!("case {} lacks prompt/license/provenance", case.id));
        }
        validate_license(&case.license)?;
        if case.license != dataset.license {
            return Err(format!(
                "case {} license differs from dataset license",
                case.id
            ));
        }
        reject_sensitive_text(&format!("case {} prompt", case.id), &case.prompt)?;
        reject_sensitive_text(&format!("case {} provenance", case.id), &case.provenance)?;
        if case.seed == 0 {
            return Err(format!("case {} seed must be non-zero", case.id));
        }
        if case.graders.is_empty() {
            return Err(format!("case {} has no deterministic graders", case.id));
        }
        if !case
            .required_capabilities
            .is_subset(&case.allowed_capabilities)
        {
            return Err(format!(
                "case {} requires a capability that is not allowed",
                case.id
            ));
        }
        let mut grader_ids = BTreeSet::new();
        for grader in &case.graders {
            if !grader_ids.insert(grader.id()) {
                return Err(format!(
                    "case {} has duplicate grader {}",
                    case.id,
                    grader.id()
                ));
            }
            if grader.id().trim().is_empty() || grader.points() == 0 {
                return Err(format!("case {} has invalid grader", case.id));
            }
            if let GraderSpecV1::Process { argv, .. } = grader {
                if argv.is_empty() {
                    return Err(format!("case {} process grader has empty argv", case.id));
                }
            }
        }
        for artifact in &case.expected_artifacts {
            safe_relative(&artifact.path)?;
            validate_sha256(&artifact.sha256)?;
        }
    }
    Ok(())
}

pub fn load_fixture(path: &Path) -> Result<FixtureV1, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("missing fixture {}: {error}", path.display()))?;
    let fixture: FixtureV1 = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid fixture {}: {error}", path.display()))?;
    if fixture.schema != FIXTURE_SCHEMA {
        return Err(format!("unsupported fixture schema {}", fixture.schema));
    }
    if fixture.license.trim().is_empty() || fixture.provenance.trim().is_empty() {
        return Err("fixture license and provenance are required".into());
    }
    validate_license(&fixture.license)?;
    reject_sensitive_text("fixture provenance", &fixture.provenance)?;
    for (path, content) in &fixture.files {
        reject_sensitive_text(&format!("fixture file {path}"), content)?;
    }
    for path in fixture.files.keys() {
        safe_relative(path)?;
    }
    Ok(fixture)
}

fn validate_license(value: &str) -> Result<(), String> {
    if !matches!(value, "Apache-2.0" | "MIT" | "CC0-1.0") {
        return Err(format!(
            "unapproved or non-canonical dataset license: {value}"
        ));
    }
    Ok(())
}

fn reject_sensitive_text(label: &str, value: &str) -> Result<(), String> {
    let lower = value.to_ascii_lowercase();
    const FORBIDDEN: &[&str] = &[
        "-----begin private key-----",
        "aws_access_key_id=",
        "authorization: bearer ",
        "ghp_",
        "github_pat_",
        "sk-live-",
        "/users/",
        "c:\\users\\",
    ];
    if let Some(pattern) = FORBIDDEN.iter().find(|pattern| lower.contains(**pattern)) {
        return Err(format!("sensitive-data leak pattern in {label}: {pattern}"));
    }
    Ok(())
}

pub fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("invalid canonical SHA-256 digest: {value}"));
    }
    Ok(())
}
