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

pub fn journal_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("store");
    target.with_file_name(format!(".{name}.migration-journal.json"))
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

pub fn read(path: &Path) -> Result<MigrationJournal, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("corrupt migration journal {}: {error}", path.display()))
}

pub fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let directory = fs::File::open(parent)
        .map_err(|error| format!("open directory {}: {error}", parent.display()))?;
    directory
        .sync_all()
        .map_err(|error| format!("fsync directory {}: {error}", parent.display()))
}
