use serde_json::Value;
use std::fs;
use std::path::Path;

/// Only a never-executed import can be refreshed. Any native turn, captured
/// extra request, task planning, or child ledger makes replacement unsafe.
pub(super) fn verify_unadopted(
    root: &Path,
    destination: &Path,
    format: super::Format,
) -> Result<(), String> {
    let adopted = destination.join("adopted");
    if adopted.try_exists().map_err(|error| {
        format!(
            "cannot inspect native adoption marker {} before refresh: {error}",
            adopted.display()
        )
    })? {
        return Err("native session has been adopted; refusing import refresh".into());
    }
    let snapshot = crate::cli::sessions::read_session_value(&destination.display().to_string())
        .map_err(|error| {
            format!(
                "cannot inspect imported native session {} before refresh: {error}",
                destination.display()
            )
        })?;
    let events = snapshot["events"].as_array().ok_or_else(|| {
        format!(
            "native ledger {} has no events array",
            destination.display()
        )
    })?;
    let reason = format!("{}-import", format.name());
    // Import writes a context snapshot; any continuation changes this shape.
    let [context] = events.as_slice() else {
        return Err("native session has been used; refusing import refresh".into());
    };
    if context["type"] != "context_snapshot"
        || context.pointer("/data/reason").and_then(Value::as_str) != Some(reason.as_str())
    {
        return Err("native session has been used; refusing import refresh".into());
    }
    let completion_path = destination.join("completion.json");
    // Refresh has the same typed, read-only admission as the native work UI.
    // The canonical reader also verifies legacy work and non-regular paths.
    let state = crate::completion::read_state(destination).map_err(|error| {
        format!(
            "cannot inspect retained native work {} before refresh: {error}",
            completion_path.display()
        )
    })?;
    let source = super::source::read(
        &destination.join(format!("artifacts/{}-source.jsonl", format.name())),
    )?;
    if state.requests.len() != source.pending.len()
        || state
            .requests
            .iter()
            .zip(&source.pending)
            .any(|(request, prompt)| request.prompt != *prompt)
        || state.revision != state.requests.len() as u64
    {
        return Err("native retained requests have changed; refusing import refresh".into());
    }
    if !state.tasks.is_empty()
        || state
            .requests
            .iter()
            .any(|request| request.planned || request.paused)
    {
        return Err("native retained work has changed; refusing import refresh".into());
    }
    for entry in fs::read_dir(root).map_err(|error| {
        format!(
            "cannot enumerate native lineage under {} before refresh: {error}",
            root.display()
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "cannot read a native lineage entry under {} before refresh: {error}",
                root.display()
            )
        })?;
        let directory = entry.path();
        let metadata = fs::metadata(&directory).map_err(|error| {
            format!(
                "cannot inspect native lineage entry {} before refresh: {error}",
                directory.display()
            )
        })?;
        if !metadata.is_dir() {
            continue;
        }
        let path = directory.join("state.json");
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot inspect native lineage state {} before refresh: {error}",
                    path.display()
                ))
            }
        };
        if !metadata.is_file() {
            return Err(format!(
                "cannot verify native lineage state {} before refresh: expected a file, observed {:?}",
                path.display(), metadata.file_type()
            ));
        }
        let bytes = fs::read(&path).map_err(|error| {
            format!(
                "cannot read native lineage state {} before refresh: {error}",
                path.display()
            )
        })?;
        let state: Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "cannot decode native lineage state {} before refresh: {error}",
                path.display()
            )
        })?;
        if state
            .pointer("/lineage/parentSession")
            .and_then(Value::as_str)
            == destination.to_str()
        {
            return Err(format!(
                "native child {} already continued this import; refusing refresh",
                path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn recover(root: &Path, destination: &Path, id: &str) -> Result<(), String> {
    let backup = root.join(format!(".previous-{id}"));
    let has_backup = backup.try_exists().map_err(|error| {
        format!(
            "cannot inspect interrupted import backup {}: {error}",
            backup.display()
        )
    })?;
    let has_destination = destination.try_exists().map_err(|error| {
        format!(
            "cannot inspect import recovery destination {}: {error}",
            destination.display()
        )
    })?;
    if has_backup && !has_destination {
        fs::rename(&backup, destination).map_err(|error| {
            format!(
                "cannot recover interrupted import from {} to {}: {error}",
                backup.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}

pub(super) fn publish(
    root: &Path,
    staging: &Path,
    destination: &Path,
    id: &str,
    format: super::Format,
) -> Result<(), String> {
    let backup = root.join(format!(".previous-{id}"));
    let has_destination = destination.try_exists().map_err(|error| {
        format!(
            "cannot inspect import publication destination {}: {error}",
            destination.display()
        )
    })?;
    if has_destination {
        verify_unadopted(root, destination, format)?;
        if backup.try_exists().map_err(|error| {
            format!(
                "cannot inspect previous import backup {}: {error}",
                backup.display()
            )
        })? {
            fs::remove_dir_all(&backup).map_err(|error| {
                format!(
                    "cannot remove previous import backup {} before replacing {}: {error}",
                    backup.display(),
                    destination.display()
                )
            })?;
        }
        fs::rename(destination, &backup).map_err(|error| {
            format!(
                "cannot retain imported session {} as previous backup {}: {error}",
                destination.display(),
                backup.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(staging, destination) {
        let original = format!(
            "cannot publish imported session from {} to {}: {error}",
            staging.display(),
            destination.display()
        );
        return Err(match recover(root, destination, id) {
            Ok(()) => original,
            Err(recovery) => {
                format!("{original}; import publication recovery also failed: {recovery}")
            }
        });
    }
    fs::File::open(root)
        .map_err(|error| {
            format!(
                "imported session is published at {}; cannot open parent {} for synchronization: {error}",
                destination.display(), root.display()
            )
        })?
        .sync_all()
        .map_err(|error| {
            format!(
                "imported session is published at {}; cannot synchronize parent {}: {error}",
                destination.display(), root.display()
            )
        })?;
    if backup.try_exists().map_err(|error| {
        format!(
            "imported session is published at {}; cannot inspect previous backup {}: {error}",
            destination.display(),
            backup.display()
        )
    })? {
        fs::remove_dir_all(&backup).map_err(|error| {
            format!(
                "imported session is published at {}; cannot remove previous backup {}: {error}",
                destination.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}
