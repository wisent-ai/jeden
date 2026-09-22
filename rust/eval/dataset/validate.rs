//! What an evaluation dataset has to satisfy before this product will run
//! it.
//!
//! Split out of `eval/dataset.rs`, which had grown past the module line cap.

use super::{safe_relative, EvalDatasetV1, FixtureV1, DATASET_SCHEMA, FIXTURE_SCHEMA};
use crate::eval::dataset::GraderSpecV1;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

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
