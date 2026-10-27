//! What a rebuild must establish before the session moves: that the manifest
//! is Jeden's own, and that the candidate it built answers as Jeden.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Whether `manifest_path`, as cargo metadata names it, is `expected`; a path
/// that no longer resolves is not.
fn same_manifest(manifest_path: &str, expected: &Path) -> bool {
    match Path::new(manifest_path).canonicalize() {
        Ok(resolved) => resolved == expected,
        Err(_unresolvable) => false,
    }
}

fn has_jeden_binary(package: &Value) -> bool {
    package
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|target| {
            target.get("name").and_then(Value::as_str) == Some("jeden")
                && target
                    .get("kind")
                    .and_then(Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
}

pub(super) fn ensure_jeden_package(metadata: &Value, manifest: &Path) -> Result<(), String> {
    let expected = manifest
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", manifest.display()))?;
    let found = metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|package| {
            package.get("name").and_then(Value::as_str) == Some("jeden")
                && package
                    .get("manifest_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| same_manifest(path, &expected))
                && has_jeden_binary(package)
        });
    if found {
        Ok(())
    } else {
        Err(format!(
            "{} is not the Jeden source manifest",
            manifest.display()
        ))
    }
}

pub(super) fn verify_candidate(executable: &Path) -> Result<(), String> {
    let mut version = Command::new(executable);
    version.arg("--version");
    let output = stado_wait::output(&mut version).map_err(|error| {
        format!(
            "rebuilt executable {} could not start: {error}",
            executable.display()
        )
    })?;
    if !output.status.success() {
        return Err(command_failure(
            &format!("{} --version", executable.display()),
            &output.stderr,
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout);
    if !version.trim().starts_with("jeden ") {
        return Err(format!(
            "rebuilt executable returned an unexpected version response: {}",
            version.trim()
        ));
    }
    Ok(())
}

pub(super) fn command_failure(command: &str, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr).trim().to_string();
    if detail.is_empty() {
        format!("{command} failed")
    } else {
        format!("{command} failed: {detail}")
    }
}
