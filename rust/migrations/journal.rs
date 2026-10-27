use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JournalStage {
    Prepared,
    BackupDurable,
    Committed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationJournal {
    pub store: String,
    pub from: u32,
    pub to: u32,
    pub target: PathBuf,
    pub staged: PathBuf,
    pub backup: Option<PathBuf>,
    pub stage: JournalStage,
}

/// The journal of the store at `target`: a hidden file beside it named
/// after it, byte for byte, so two stores never share one; a target that
/// names no file (a directory path ending in `..` or a root) keeps its
/// journal inside itself.
pub fn journal_path(target: &Path) -> PathBuf {
    match target.file_name() {
        Some(name) => {
            let mut journal = std::ffi::OsString::from(".");
            journal.push(name);
            journal.push(".migration-journal.json");
            target.with_file_name(journal)
        }
        None => target.join(".migration-journal.json"),
    }
}

pub fn write_durable(path: &Path, journal: &MigrationJournal) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(journal).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all()
        .map_err(|error| format!("fsync {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("commit journal {}: {error}", path.display()))?;
    sync_parent(path)
}

/// The journal at `path`, or `None` only when it does not exist: a journal
/// that cannot be read is refused, not passed over as no interrupted
/// migration.
pub fn read(path: &Path) -> Result<Option<MigrationJournal>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("corrupt migration journal {}: {error}", path.display()))
}

/// Make the entry of `path` in its directory durable. A bare relative file
/// name lives in the working directory, whose name is `.`; a path with no
/// directory at all (a root) is refused, since nothing holds its entry.
pub fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => return Err(format!("{} has no directory whose entry could be synced", path.display())),
    };
    let directory = fs::File::open(parent)
        .map_err(|error| format!("open directory {}: {error}", parent.display()))?;
    directory
        .sync_all()
        .map_err(|error| format!("fsync directory {}: {error}", parent.display()))
}
