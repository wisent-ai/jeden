mod replacement;
mod source;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) use source::Format;

/// `jeden import <path>... [--refresh] [--json]`: each path is a transcript
/// another harness wrote, or a directory to scan for them. Which harness is
/// read from the file, never from the command. A one-format command with a
/// plan file lived here once and was refused by the operator; this is the
/// general surface that replaced it.
pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
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
        return Err("Usage: jeden import <path>... [--refresh] [--json]".into());
    }
    let result = import_paths(&paths, refresh)?;
    serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
}

/// Import does not stop the other harness or execute work. The desktop adopts
/// these ledgers only after source ownership has been handed over, never while
/// both can write.
pub(crate) fn import_paths(paths: &[PathBuf], refresh: bool) -> Result<Value, String> {
    let root = crate::session_root();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let lock_path = root.with_extension("import.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("cannot open import lock {}: {e}", lock_path.display()))?;
    lock.try_lock()
        .map_err(|e| format!("another import holds {}: {e}", lock_path.display()))?;
    let mut sessions = Vec::new();
    let mut failures = Vec::new();
    let mut existing = 0;
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
            Ok((record, was_existing)) => {
                existing += usize::from(was_existing);
                sessions.push(record);
            }
            Err(error) => failures.push(json!({"sourcePath":source_path,"error":error})),
        }
    }
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
            .map_err(|e| e.to_string())?
            .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                collect(&entry, sources)?;
            } else if source::detect(&entry).is_some() {
                sources.push(entry);
            }
        }
        return Ok(());
    }
    if source::detect(path).is_none() {
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
    let before = fs::metadata(&source_path).map_err(|e| e.to_string())?;
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
    if manifest.exists() {
        let result: Value =
            serde_json::from_slice(&fs::read(&manifest).map_err(|e| e.to_string())?)
                .map_err(|e| format!("invalid import receipt: {e}"))?;
        let unchanged = result["sourceBytes"].as_u64() == Some(before.len())
            && result["sourceModified"].as_str() == Some(&modified(&before)?);
        if unchanged && !refresh {
            return Ok((result, true));
        }
        if !refresh {
            return Err(
                "source changed since import; use --refresh only for a never-adopted import".into(),
            );
        }
        replacement::verify_unadopted(root, &destination, source.format)?;
    } else if destination.exists() {
        return Err(format!(
            "incomplete migration at {}; source is intact",
            destination.display()
        ));
    }
    let staging = root.join(format!(".import-{id}"));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(staging.join("artifacts")).map_err(|e| e.to_string())?;
    let result = (|| {
        let provenance = staging.join(format!("artifacts/{format}-source.jsonl"));
        fs::copy(&source_path, &provenance).map_err(|e| format!("source snapshot failed: {e}"))?;
        File::open(&provenance)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        let after = fs::metadata(&source_path).map_err(|e| e.to_string())?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err("source changed during import; stop its writer through its normal interface and retry".into());
        }
        let timestamp = crate::agent::now_stamp();
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
            json!({"reason":format!("{format}-import"), "messages":source.messages}),
        )?;
        for request in &source.pending {
            crate::completion::capture_request(&staging, &cwd, request)?;
        }
        let record = json!({"format":format,"sourceSession":source.id,"sourcePath":source_path,
            "sessionPath":destination,"cwd":cwd,"title":source.title,
            "interrupted":!source.pending.is_empty(),"sourceBytes":before.len(),
            "sourceModified":modified(&before)?,"importedAt":timestamp});
        write_json(&staging.join("import.json"), &record)?;
        File::open(&staging)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        replacement::publish(root, &staging, &destination, &id, source.format)?;
        Ok((record, false))
    })();
    if result.is_err() && staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|e| format!("migration failed; staging cleanup also failed: {e}"))?;
    }
    result
}

fn modified(metadata: &fs::Metadata) -> Result<String, String> {
    metadata
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_nanos().to_string())
        .map_err(|e| e.to_string())
}
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = File::create(path).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

pub(crate) fn mark_adopted(source: &Path) -> Result<(), String> {
    if !source.join("import.json").exists() {
        return Ok(());
    }
    let root = source.parent().ok_or("import has no parent directory")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.with_extension("import.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock()
        .map_err(|e| format!("cannot claim imported session: {e}"))?;
    File::create(source.join("adopted"))
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("cannot record import ownership: {e}"))
}
