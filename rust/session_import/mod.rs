mod source;
mod replacement;

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanEntry {
    session_file: PathBuf,
    cwd: PathBuf,
    terminal_name: String,
}
#[derive(Deserialize)]
struct Plan { sessions: Vec<PlanEntry> }

pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
    let refresh = args.positionals.last().map(String::as_str) == Some("--refresh");
    let arguments = &args.positionals[..args.positionals.len() - usize::from(refresh)];
    if arguments.len() != 2 || arguments[0] != "--plan" {
        return Err("Usage: jeden import-omp --plan <sessions.json> [--refresh] [--json]".into());
    }
    let result = import_plan(Path::new(&arguments[1]), refresh)?;
    serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
}

/// Import does not stop OMP or execute work. The desktop adopts these ledgers
/// only after source ownership has been handed over, never while both can write.
pub(crate) fn import_plan(path: &Path, refresh: bool) -> Result<Value, String> {
    let plan: Plan = serde_json::from_slice(&fs::read(path)
        .map_err(|e| format!("cannot read plan {}: {e}", path.display()))?)
        .map_err(|e| format!("invalid plan {}: {e}", path.display()))?;
    if plan.sessions.is_empty() { return Err("migration plan contains no sessions".into()); }
    let root = crate::session_root();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let lock_path = root.with_extension("omp-import.lock");
    let lock = OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path)
        .map_err(|e| format!("cannot open migration lock {}: {e}", lock_path.display()))?;
    lock.try_lock().map_err(|e| format!("another OMP import holds {}: {e}", lock_path.display()))?;
    let mut sessions = Vec::new();
    let mut failures = Vec::new();
    let mut existing = 0;
    for entry in plan.sessions {
        match import_entry(&root, &entry, refresh) {
            Ok((record, was_existing)) => { existing += usize::from(was_existing); sessions.push(record); }
            Err(error) => failures.push(json!({"sourcePath":entry.session_file,"error":error})),
        }
    }
    Ok(json!({"imported":sessions.len()-existing, "existing":existing,
        "sessions":sessions, "failures":failures}))
}

fn import_entry(root: &Path, entry: &PlanEntry, refresh: bool) -> Result<(Value, bool), String> {
    let source_path = fs::canonicalize(&entry.session_file)
        .map_err(|e| format!("cannot open source {}: {e}", entry.session_file.display()))?;
    let before = fs::metadata(&source_path).map_err(|e| e.to_string())?;
    let source = source::read(&source_path)?;
    let cwd = fs::canonicalize(&entry.cwd).map_err(|e| format!("workspace unavailable: {e}"))?;
    if !cwd.is_dir() { return Err(format!("workspace is not a directory: {}", cwd.display())); }
    let declared = fs::canonicalize(&source.cwd).map_err(|e| format!("source workspace unavailable: {e}"))?;
    if cwd != declared { return Err(format!("plan workspace {} differs from source {}", cwd.display(), declared.display())); }
    let id = format!("omp-{}", hex::encode(Sha256::digest(source.id.as_bytes())));
    let destination = root.join(&id);
    replacement::recover(root, &destination, &id)?;
    let manifest = destination.join("import.json");
    if manifest.exists() {
        let result: Value = serde_json::from_slice(&fs::read(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| format!("invalid import receipt: {e}"))?;
        let unchanged = result["sourceBytes"].as_u64() == Some(before.len())
            && result["sourceModified"].as_str() == Some(&modified(&before)?);
        if unchanged && !refresh { return Ok((result, true)); }
        if !refresh {
            return Err("source changed since import; use --refresh only for a never-adopted import".into());
        }
        replacement::verify_unadopted(root, &destination)?;
    } else if destination.exists() {
        return Err(format!("incomplete migration at {}; source is intact", destination.display()));
    }
    let staging = root.join(format!(".import-{id}"));
    if staging.exists() { fs::remove_dir_all(&staging).map_err(|e| e.to_string())?; }
    fs::create_dir_all(staging.join("artifacts")).map_err(|e| e.to_string())?;
    let result = (|| {
        let provenance = staging.join("artifacts/omp-source.jsonl");
        fs::copy(&source_path, &provenance).map_err(|e| format!("source snapshot failed: {e}"))?;
        File::open(&provenance).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        let after = fs::metadata(&source_path).map_err(|e| e.to_string())?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err("source changed during import; stop its writer through its normal interface and retry".into());
        }
        let timestamp = crate::agent::now_stamp();
        write_json(&staging.join("state.json"), &json!({
            "version":crate::cli::sessions::SESSION_LEDGER_VERSION,"id":id,
            "cwd":cwd,"startedAt":timestamp,"activeLeaf":null,"lineage":null}))?;
        crate::cli::sessions::append_ledger_entry(&staging, timestamp.clone(), "context_snapshot",
            json!({"reason":"omp-import", "messages":source.messages}))?;
        for request in &source.pending {
            crate::completion::capture_request(&staging, &cwd, request)?;
        }
        let title = if source.title.is_empty() { &entry.terminal_name } else { &source.title };
        let record = json!({"sourceSession":source.id,"sourcePath":source_path,
            "sessionPath":destination,"cwd":cwd,"title":title,
            "interrupted":!source.pending.is_empty(),"sourceBytes":before.len(),
            "sourceModified":modified(&before)?,"importedAt":timestamp});
        write_json(&staging.join("import.json"), &record)?;
        File::open(&staging).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        replacement::publish(root, &staging, &destination, &id)?;
        Ok((record, false))
    })();
    if result.is_err() && staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| format!("migration failed; staging cleanup also failed: {e}"))?;
    }
    result
}

fn modified(metadata: &fs::Metadata) -> Result<String, String> {
    metadata.modified().map_err(|e| e.to_string())?.duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_nanos().to_string()).map_err(|e| e.to_string())
}
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = File::create(path).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n").and_then(|_| file.sync_all()).map_err(|e| e.to_string())
}

pub(crate) fn mark_adopted(source: &Path) -> Result<(), String> {
    if !source.join("import.json").exists() { return Ok(()); }
    let root = source.parent().ok_or("import has no parent directory")?;
    let lock = OpenOptions::new().create(true).truncate(false).write(true)
        .open(root.with_extension("omp-import.lock")).map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| format!("cannot claim imported session: {e}"))?;
    File::create(source.join("adopted")).and_then(|f| f.sync_all())
        .map_err(|e| format!("cannot record import ownership: {e}"))
}
