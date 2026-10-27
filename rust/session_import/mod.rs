mod replacement;
mod source;
mod storage;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use storage::{modified, write_json};

pub(crate) use source::Format;

/// `jeden import <path>... [--refresh] [--json]`: each path is a transcript
/// another harness wrote, or a directory to scan for them. Which harness is
/// read from the file, never from the command. A one-format command with a
/// plan file lived here once and was refused by the operator; this is the
/// general surface that replaced it.
pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
    const USAGE: &str = "Usage: jeden import <path>... [--refresh] [--json]";
    if let Some(option) = args
        .positionals
        .iter()
        .find(|argument| argument.starts_with('-') && *argument != "--refresh")
    {
        return Err(crate::cli::invocation::refusal::usage(format!(
            "unknown option for jeden import: {option}\n{USAGE}"
        )));
    }
    let refresh = args
        .positionals
        .iter()
        .any(|argument| argument == "--refresh");
    let paths: Vec<PathBuf> = args
        .positionals
        .iter()
        .filter(|argument| *argument != "--refresh")
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Err(crate::cli::invocation::refusal::usage(USAGE));
    }
    let result = import_paths(&paths, refresh)?;
    serde_json::to_string_pretty(&result)
        .map_err(|error| format!("the import report cannot be written as JSON: {error}"))
}

/// Import does not stop the other harness or execute work. The desktop adopts
/// these ledgers only after source ownership has been handed over, never while
/// both can write.
pub(crate) fn import_paths(paths: &[PathBuf], refresh: bool) -> Result<Value, String> {
    let root = crate::session_root();
    fs::create_dir_all(&root).map_err(|error| {
        format!("the session root {} cannot be created: {error}", root.display())
    })?;
    let lock_path = root.with_extension("import.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("cannot open import lock {}: {e}", lock_path.display()))?;
    lock.try_lock()
        .map_err(|e| format!("another import holds {}: {e}", lock_path.display()))?;
    let mut imported = Vec::new();
    let mut failures = Vec::new();
    let mut sources = Vec::new();
    for path in paths {
        match collect(path, &mut sources) {
            Ok(()) => {}
            Err(error) => failures.push(json!({"sourcePath":path,"error":error})),
        }
    }
    if sources.is_empty() && failures.is_empty() {
        let named: Vec<String> = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        return Err(format!("no sessions to import under {}", named.join(", ")));
    }
    for source_path in sources {
        match import_source(&root, &source_path, refresh) {
            Ok(outcome) => imported.push(outcome),
            Err(error) => failures.push(json!({"sourcePath":source_path,"error":error})),
        }
    }
    let existing = imported
        .iter()
        .filter(|(_, was_existing)| *was_existing)
        .count();
    let sessions: Vec<_> = imported.into_iter().map(|(record, _)| record).collect();
    Ok(
        json!({"imported":sessions.len()-existing, "existing":existing,
        "sessions":sessions, "failures":failures}),
    )
}

/// A file named outright must be a transcript; a directory is walked and only
/// the transcripts inside it count, because a session root also holds locks,
/// state files and other harnesses' leftovers.
fn collect(path: &Path, sources: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("cannot open source {}: {e}", path.display()))?;
    if metadata.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .map_err(|error| format!("cannot list source directory {}: {error}", path.display()))?
            .map(|entry| {
                entry.map(|entry| entry.path()).map_err(|error| {
                    format!("an entry of {} cannot be read: {error}", path.display())
                })
            })
            .collect::<Result<_, _>>()?;
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                collect(&entry, sources)?;
            } else if entry.is_file() {
                // Only a file that is no supported transcript is left out. One that
                // cannot be read is kept: its import reads it again and names
                // it under failures with its cause, rather than skipping a
                // transcript in silence as "not a transcript".
                if !matches!(source::detect(&entry), Ok(None)) {
                    sources.push(entry);
                }
            }
        }
        return Ok(());
    }
    if source::detect(path)?.is_none() {
        return Err(format!(
            "{}: not a transcript of a supported harness (supported: {})",
            path.display(),
            Format::SUPPORTED
        ));
    }
    sources.push(path.to_path_buf());
    Ok(())
}

fn import_source(root: &Path, path: &Path, refresh: bool) -> Result<(Value, bool), String> {
    let source_path = fs::canonicalize(path)
        .map_err(|e| format!("cannot open source {}: {e}", path.display()))?;
    let before = fs::metadata(&source_path)
        .map_err(|error| format!("cannot read source {}: {error}", source_path.display()))?;
    let source = source::read(&source_path)?;
    let format = source.format.name();
    let cwd =
        fs::canonicalize(&source.cwd).map_err(|e| format!("source workspace unavailable: {e}"))?;
    if !cwd.is_dir() {
        return Err(format!(
            "source workspace is not a directory: {}",
            cwd.display()
        ));
    }
    let id = format!(
        "{format}-{}",
        hex::encode(Sha256::digest(source.id.as_bytes()))
    );
    let destination = root.join(&id);
    replacement::recover(root, &destination, &id)?;
    let manifest = destination.join("import.json");
    // Each of these decides whether an import is replaced, refused or
    // started over; a path that cannot be examined is refused with its cause,
    // never read as absent.
    let present = crate::tool_runtime::runtime_ops::platform::presence::present;
    if present(&manifest)? {
        let result: Value =
            serde_json::from_slice(&fs::read(&manifest).map_err(|error| {
                format!("cannot read import receipt {}: {error}", manifest.display())
            })?)
            .map_err(|e| format!("invalid import receipt {}: {e}", manifest.display()))?;
        let unchanged = result["sourceBytes"].as_u64() == Some(before.len())
            && result["sourceModified"].as_str() == Some(&modified(&source_path, &before)?);
        if unchanged && !refresh {
            return Ok((result, true));
        }
        if !refresh {
            return Err(
                "source changed since import; use --refresh only for a never-adopted import".into(),
            );
        }
        replacement::verify_unadopted(root, &destination, source.format)?;
    } else if present(&destination)? {
        return Err(format!(
            "incomplete migration at {}; source is intact",
            destination.display()
        ));
    }
    let staging = root.join(format!(".import-{id}"));
    if present(&staging)? {
        fs::remove_dir_all(&staging).map_err(|error| {
            format!(
                "the leftover import staging {} cannot be removed: {error}",
                staging.display()
            )
        })?;
    }
    let artifacts = staging.join("artifacts");
    let result = (|| {
        fs::create_dir_all(&artifacts).map_err(|error| {
            format!(
                "cannot create import artifacts directory {}: {error}",
                artifacts.display()
            )
        })?;
        let provenance = staging.join(format!("artifacts/{format}-source.jsonl"));
        fs::copy(&source_path, &provenance).map_err(|error| {
            format!(
                "cannot copy source {} to import snapshot {}: {error}",
                source_path.display(),
                provenance.display()
            )
        })?;
        File::open(&provenance)
            .map_err(|error| {
                format!(
                    "cannot open import snapshot {} for synchronization: {error}",
                    provenance.display()
                )
            })?
            .sync_all()
            .map_err(|error| {
                format!(
                    "cannot synchronize import snapshot {}: {error}",
                    provenance.display()
                )
            })?;
        let after = fs::metadata(&source_path).map_err(|error| {
            format!(
                "cannot inspect source {} after copying its snapshot: {error}",
                source_path.display()
            )
        })?;
        if before.len() != after.len()
            || modified(&source_path, &before)? != modified(&source_path, &after)?
        {
            return Err("source changed during import; stop its writer through its normal interface and retry".into());
        }
        let timestamp = crate::agent::now_stamp()
            .map_err(|error| format!("the imported session cannot be dated: {error}"))?;
        write_json(
            &staging.join("state.json"),
            &json!({
            "version":crate::cli::sessions::SESSION_LEDGER_VERSION,"id":id,
            "cwd":cwd,"startedAt":timestamp,"activeLeaf":null,"lineage":null}),
        )?;
        crate::cli::sessions::append_ledger_entry(
            &staging,
            timestamp.clone(),
            "context_snapshot",
            json!({"reason":format!("{format}-import"), "cwd":cwd, "messages":source.messages}),
        )?;
        for request in &source.pending {
            crate::completion::capture_request(&staging, &cwd, request)?;
        }
        let record = json!({"format":format,"sourceSession":source.id,"sourcePath":source_path,
            "sessionPath":destination,"cwd":cwd,"title":source.title,
            "interrupted":!source.pending.is_empty(),"sourceBytes":before.len(),
            "sourceModified":modified(&source_path, &before)?,"importedAt":timestamp});
        write_json(&staging.join("import.json"), &record)?;
        File::open(&staging)
            .map_err(|error| {
                format!(
                    "cannot open import staging {} for synchronization: {error}",
                    staging.display()
                )
            })?
            .sync_all()
            .map_err(|error| {
                format!(
                    "cannot synchronize import staging {}: {error}",
                    staging.display()
                )
            })?;
        replacement::publish(root, &staging, &destination, &id, source.format)?;
        Ok((record, false))
    })();
    result.map_err(|original| storage::rollback(&staging, original))
}

pub(crate) fn mark_adopted(source: &Path) -> Result<(), String> {
    // An import whose receipt cannot be examined is refused, not left
    // unowned: read as absent, the session was adopted with no record, and
    // a later --refresh replaced it.
    if !crate::tool_runtime::runtime_ops::platform::presence::present(&source.join("import.json"))? {
        return Ok(());
    }
    let root = source.parent().ok_or("import has no parent directory")?;
    let lock_path = root.with_extension("import.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("cannot open import lock {}: {error}", lock_path.display()))?;
    lock.lock()
        .map_err(|e| format!("cannot claim imported session: {e}"))?;
    File::create(source.join("adopted"))
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("cannot record import ownership: {e}"))
}
