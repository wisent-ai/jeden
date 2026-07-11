use super::snapshot::parse_tree;
use super::{CasError, Digest, LocalCas};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug)]
pub struct GcOptions {
    /// Young objects are retained to avoid racing writers and newly published roots.
    pub minimum_age: Duration,
    pub dry_run: bool,
}
impl Default for GcOptions {
    fn default() -> Self {
        Self {
            minimum_age: Duration::from_secs(60 * 60),
            dry_run: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GcReport {
    pub reachable_objects: u64,
    pub removed_objects: u64,
    pub reclaimed_bytes: u64,
    pub retained_young_objects: u64,
}

/// Marks roots and all tree descendants, then removes only old, well-named,
/// hash-valid objects. Unknown files and corrupt objects are retained.
pub fn collect_garbage(
    cas: &LocalCas,
    roots: impl IntoIterator<Item = Digest>,
    options: GcOptions,
) -> Result<GcReport, CasError> {
    let mut reachable = HashSet::new();
    let mut pending: Vec<Digest> = roots.into_iter().collect();
    while let Some(digest) = pending.pop() {
        if !reachable.insert(digest) {
            continue;
        }
        let bytes = cas.get(digest)?;
        if let Some(tree) = parse_tree(&bytes) {
            pending.extend(tree.entries.into_iter().map(|entry| entry.digest));
        }
    }

    let now = SystemTime::now();
    let mut report = GcReport {
        reachable_objects: reachable.len() as u64,
        ..GcReport::default()
    };
    for shard in read_directory_if_present(&cas.objects_dir())? {
        let shard = shard
            .map_err(|error| CasError::io("read CAS shard entry", cas.objects_dir(), error))?;
        let shard_name = match shard.file_name().to_str() {
            Some(name) if is_lower_hex(name, 2) => name.to_owned(),
            _ => continue,
        };
        let shard_type = shard
            .file_type()
            .map_err(|error| CasError::io("inspect CAS shard", shard.path(), error))?;
        if !shard_type.is_dir() {
            continue;
        }
        for object in fs::read_dir(shard.path())
            .map_err(|error| CasError::io("read CAS shard", shard.path(), error))?
        {
            let object = object
                .map_err(|error| CasError::io("read CAS object entry", shard.path(), error))?;
            let suffix = match object.file_name().to_str() {
                Some(name) if is_lower_hex(name, 62) => name.to_owned(),
                _ => continue,
            };
            let digest: Digest = match format!("{shard_name}{suffix}").parse() {
                Ok(digest) => digest,
                Err(_) => continue,
            };
            if reachable.contains(&digest) {
                continue;
            }
            let path = object.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| CasError::io("inspect garbage candidate", &path, error))?;
            if !metadata.file_type().is_file() {
                continue;
            }
            let age = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok());
            if age.map_or(true, |age| age < options.minimum_age) {
                report.retained_young_objects += 1;
                continue;
            }
            // Never turn corruption into data loss: get() must authenticate the candidate first.
            if cas.get(digest).is_err() {
                continue;
            }
            if !options.dry_run {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(CasError::io("remove unreferenced CAS object", path, error))
                    }
                }
            }
            report.removed_objects += 1;
            report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(metadata.len());
        }
    }
    Ok(report)
}

impl LocalCas {
    pub fn collect_garbage(
        &self,
        roots: impl IntoIterator<Item = Digest>,
        options: GcOptions,
    ) -> Result<GcReport, CasError> {
        collect_garbage(self, roots, options)
    }
}

fn read_directory_if_present(path: &Path) -> Result<fs::ReadDir, CasError> {
    fs::read_dir(path).map_err(|error| CasError::io("read CAS objects directory", path, error))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
