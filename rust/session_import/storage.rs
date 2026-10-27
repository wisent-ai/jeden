use serde_json::Value;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub(super) fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = File::create(path)
        .map_err(|error| format!("cannot create import JSON {}: {error}", path.display()))?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("cannot write import JSON {}: {error}", path.display()))?;
    file.write_all(b"\n")
        .map_err(|error| format!("cannot finish import JSON line {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot synchronize import JSON {}: {error}", path.display()))
}

pub(super) fn modified(path: &Path, metadata: &fs::Metadata) -> Result<String, String> {
    let modified = metadata.modified().map_err(|error| {
        format!(
            "cannot read source modification time {}: {error}",
            path.display()
        )
    })?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos().to_string())
        .map_err(|error| {
            format!(
                "cannot represent source modification time {modified:?} for {}: {error}",
                path.display()
            )
        })
}

/// A failed cleanup is secondary evidence, never a replacement for the failure
/// that prevented publication. Successful cleanup keeps the original allocation.
pub(super) fn rollback(staging: &Path, original: String) -> String {
    match staging.try_exists() {
        Ok(false) => original,
        Ok(true) => match fs::remove_dir_all(staging) {
            Ok(()) => original,
            Err(cleanup) => format!(
                "{original}; removing failed import staging {} also failed: {cleanup}",
                staging.display()
            ),
        },
        Err(inspection) => format!(
            "{original}; inspecting import staging {} for cleanup also failed: {inspection}",
            staging.display()
        ),
    }
}
