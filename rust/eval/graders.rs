use super::dataset::{safe_relative, GraderSpecV1};
use super::metrics::GraderEvidenceV1;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub const GRADER_IMPLEMENTATION_REVISION: &str = "jeden.deterministic-graders.v1";

pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

fn canonical<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

fn evidence(
    spec: &GraderSpecV1,
    passed: bool,
    detail: String,
    payload: &[u8],
) -> Result<GraderEvidenceV1, String> {
    let spec_bytes = canonical(spec)?;
    Ok(GraderEvidenceV1 {
        grader_id: spec.id().into(),
        grader_digest: sha256(
            [
                GRADER_IMPLEMENTATION_REVISION.as_bytes(),
                spec_bytes.as_slice(),
            ]
            .concat(),
        ),
        earned: if passed { spec.points() } else { 0 },
        possible: spec.points(),
        passed,
        hard: spec.hard(),
        evidence_digest: sha256(payload),
        detail,
    })
}

pub fn grade(
    spec: &GraderSpecV1,
    workspace: &Path,
    artifacts: &Path,
    isolated_env: &BTreeMap<String, String>,
) -> Result<GraderEvidenceV1, String> {
    match spec {
        GraderSpecV1::FileEquals { path, content, .. } => {
            let file = workspace.join(safe_relative(path)?);
            let actual = fs::read(&file).map_err(|error| {
                format!(
                    "grader {} missing file {}: {error}",
                    spec.id(),
                    file.display()
                )
            })?;
            evidence(
                spec,
                actual == content.as_bytes(),
                format!("file {path} byte equality"),
                &actual,
            )
        }
        GraderSpecV1::FileContains { path, needle, .. } => {
            let file = workspace.join(safe_relative(path)?);
            let actual = fs::read(&file).map_err(|error| {
                format!(
                    "grader {} missing file {}: {error}",
                    spec.id(),
                    file.display()
                )
            })?;
            let passed = String::from_utf8_lossy(&actual).contains(needle);
            evidence(
                spec,
                passed,
                format!("file {path} contains declared UTF-8 text"),
                &actual,
            )
        }
        GraderSpecV1::FileAbsent { path, .. } => {
            let file = workspace.join(safe_relative(path)?);
            let passed = !file.exists();
            evidence(
                spec,
                passed,
                format!("file {path} is absent"),
                if passed { b"absent" } else { b"present" },
            )
        }
        GraderSpecV1::JsonEquals { path, expected, .. } => {
            let file = workspace.join(safe_relative(path)?);
            let bytes = fs::read(&file).map_err(|error| {
                format!(
                    "grader {} missing JSON {}: {error}",
                    spec.id(),
                    file.display()
                )
            })?;
            let actual: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("grader {} invalid JSON: {error}", spec.id()))?;
            let canonical_actual = canonical(&actual)?;
            evidence(
                spec,
                &actual == expected,
                format!("JSON value equality for {path}"),
                &canonical_actual,
            )
        }
        GraderSpecV1::JsonSchema { path, schema, .. } => {
            let file = workspace.join(safe_relative(path)?);
            let bytes = fs::read(&file).map_err(|error| {
                format!(
                    "grader {} missing JSON {}: {error}",
                    spec.id(),
                    file.display()
                )
            })?;
            let actual: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("grader {} invalid JSON: {error}", spec.id()))?;
            validate_schema(schema, &actual, "$").map_or_else(
                |detail| {
                    evidence(
                        spec,
                        false,
                        format!("schema mismatch for {path}: {detail}"),
                        &bytes,
                    )
                },
                |_| evidence(spec, true, format!("JSON Schema match for {path}"), &bytes),
            )
        }
        GraderSpecV1::Process {
            argv,
            expected_exit,
            stdout_contains,
            ..
        } => {
            let executable = argv
                .first()
                .ok_or_else(|| format!("grader {} has empty argv", spec.id()))?;
            let mut command = Command::new(executable);
            command
                .args(&argv[1..])
                .current_dir(workspace)
                .env_clear()
                .envs(isolated_env)
                .stdin(Stdio::null())
                .stderr(Stdio::piped())
                .stdout(Stdio::piped());
            let output = command.output().map_err(|error| {
                format!("grader {} process failed to start: {error}", spec.id())
            })?;
            let exit = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let passed = exit == *expected_exit
                && stdout_contains
                    .as_ref()
                    .map(|needle| stdout.contains(needle))
                    .unwrap_or(true);
            let mut payload = output.stdout;
            payload.extend_from_slice(&output.stderr);
            evidence(
                spec,
                passed,
                format!("process exit={exit}, expected={expected_exit}"),
                &payload,
            )
        }
        GraderSpecV1::ArtifactSha256 {
            path,
            sha256: expected,
            ..
        } => {
            let file = artifacts.join(safe_relative(path)?);
            let bytes = fs::read(&file).map_err(|error| {
                format!(
                    "grader {} missing artifact {}: {error}",
                    spec.id(),
                    file.display()
                )
            })?;
            let actual = sha256(&bytes);
            evidence(
                spec,
                &actual == expected,
                format!("artifact {path} sha256={actual}"),
                &bytes,
            )
        }
    }
}

fn validate_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    location: &str,
) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| "schema must be an object".to_string())?;
    const SUPPORTED: &[&str] = &[
        "$schema",
        "$id",
        "title",
        "description",
        "type",
        "const",
        "required",
        "properties",
        "items",
        "additionalProperties",
    ];
    if let Some(keyword) = object.keys().find(|key| !SUPPORTED.contains(&key.as_str())) {
        return Err(format!("unsupported schema keyword {keyword}"));
    }
    if let Some(expected) = object.get("const") {
        if expected != value {
            return Err(format!("{location} does not equal const"));
        }
    }
    if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str) {
        let matches = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => return Err(format!("unsupported schema type {kind}")),
        };
        if !matches {
            return Err(format!("{location} is not {kind}"));
        }
    }
    if let Some(required) = object.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| "required must be an array".to_string())?;
        let instance = value
            .as_object()
            .ok_or_else(|| format!("{location} is not an object"))?;
        for key in required {
            let key = key
                .as_str()
                .ok_or_else(|| "required entries must be strings".to_string())?;
            if !instance.contains_key(key) {
                return Err(format!("{location} lacks required property {key}"));
            }
        }
    }
    if let Some(properties) = object.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| "properties must be an object".to_string())?;
        let instance = value
            .as_object()
            .ok_or_else(|| format!("{location} is not an object"))?;
        for (key, child_schema) in properties {
            if let Some(child) = instance.get(key) {
                validate_schema(child_schema, child, &format!("{location}.{key}"))?;
            }
        }
        if object.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
            if let Some(key) = instance.keys().find(|key| !properties.contains_key(*key)) {
                return Err(format!("{location} has undeclared property {key}"));
            }
        }
    }
    if let Some(items) = object.get("items") {
        for (index, child) in value
            .as_array()
            .ok_or_else(|| format!("{location} is not an array"))?
            .iter()
            .enumerate()
        {
            validate_schema(items, child, &format!("{location}[{index}]"))?;
        }
    }
    Ok(())
}
