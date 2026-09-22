//! Giving each evaluation case a workspace of its own that nothing outside it
//! can reach.
//!
//! Split out of `eval/runner.rs`, which had grown past the module line cap.

use super::super::dataset::{safe_relative, EvalCaseV1, FixtureV1};
use super::IsolatedRunV1;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn resolve_repo_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let candidate = root.join(safe_relative(relative)?);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "cannot resolve repository artifact {}: {error}",
            candidate.display()
        )
    })?;
    if !canonical.starts_with(root) {
        return Err(format!("repository artifact escapes root: {relative}"));
    }
    Ok(canonical)
}

pub(super) fn run_key(
    case: &EvalCaseV1,
    dataset: &str,
    fixture: &str,
    grader: &str,
    code: &str,
    catalog: &str,
    policy: &str,
) -> String {
    let mut digest = Sha256::new();
    for part in [
        case.id.as_str(),
        &case.seed.to_string(),
        dataset,
        fixture,
        grader,
        code,
        catalog,
        policy,
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

pub(super) fn isolated_run(output_root: &Path, run_key: &str) -> Result<IsolatedRunV1, String> {
    let root = output_root.join("runs").join(run_key);
    let home = root.join("home");
    let session = root.join("session");
    let memory = root.join("memory");
    let quality_db = root.join("quality");
    let workspace = root.join("workspace");
    let artifacts = root.join("artifacts");
    for path in [
        &home,
        &session,
        &memory,
        &quality_db,
        &workspace,
        &artifacts,
    ] {
        fs::create_dir_all(path)
            .map_err(|error| format!("cannot create isolated path {}: {error}", path.display()))?;
    }
    let environment = BTreeMap::from([
        ("HOME".into(), home.display().to_string()),
        ("JEDEN_SESSION_DIR".into(), session.display().to_string()),
        ("JEDEN_MEMORY_DIR".into(), memory.display().to_string()),
        (
            "JEDEN_QUALITY_DB_DIR".into(),
            quality_db.display().to_string(),
        ),
        ("JEDEN_WORKSPACE".into(), workspace.display().to_string()),
        ("JEDEN_ARTIFACT_DIR".into(), artifacts.display().to_string()),
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("TZ".into(), "UTC".into()),
        ("LANG".into(), "C".into()),
    ]);
    Ok(IsolatedRunV1 {
        run_key: run_key.into(),
        root,
        home,
        session,
        memory,
        quality_db,
        workspace,
        artifacts,
        environment,
    })
}

pub(super) fn materialize_fixture(fixture: &FixtureV1, workspace: &Path) -> Result<(), String> {
    for (relative, content) in &fixture.files {
        let target = workspace.join(safe_relative(relative)?);
        if target.exists() {
            let existing = fs::read(&target).map_err(|error| error.to_string())?;
            if existing != content.as_bytes() {
                return Err(format!(
                    "partially materialized fixture differs at {relative}"
                ));
            }
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        atomic_write_new(&target, content.as_bytes())?;
    }
    Ok(())
}

pub(super) fn atomic_write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
