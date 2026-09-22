mod safety;

use super::{CasError, Digest, LocalCas};
use safety::{ensure_safe_destination, validate_component, validate_tree};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub(super) const TREE_SCHEMA: &str = "jeden.merkle-tree.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MerkleTree {
    pub(super) schema: String,
    pub entries: Vec<MerkleEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MerkleEntry {
    pub name: String,
    pub kind: EntryKind,
    pub digest: Digest,
    #[serde(default, skip_serializing_if = "is_false")]
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Directory,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub struct SnapshotBuilder<'a> {
    cas: &'a LocalCas,
}
impl<'a> SnapshotBuilder<'a> {
    pub fn new(cas: &'a LocalCas) -> Self {
        Self { cas }
    }

    /// Stores every file and directory node and returns the root tree digest.
    pub fn build(&self, root: impl AsRef<Path>) -> Result<Digest, CasError> {
        let root = root.as_ref();
        let metadata = fs::symlink_metadata(root)
            .map_err(|error| CasError::io("inspect snapshot root", root, error))?;
        if !metadata.file_type().is_dir() {
            return Err(CasError::UnsupportedEntry(root.to_path_buf()));
        }
        self.build_directory(root)
    }

    fn build_directory(&self, directory: &Path) -> Result<Digest, CasError> {
        let mut children = Vec::new();
        for child in fs::read_dir(directory)
            .map_err(|error| CasError::io("read snapshot directory", directory, error))?
        {
            let child = child
                .map_err(|error| CasError::io("read snapshot directory entry", directory, error))?;
            let name = child.file_name().into_string().map_err(|_| {
                CasError::InvalidPath(format!("{} contains a non-UTF-8 name", directory.display()))
            })?;
            validate_component(&name)?;
            let path = child.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| CasError::io("inspect snapshot entry", &path, error))?;
            let file_type = metadata.file_type();
            let (kind, digest, executable) = if file_type.is_file() {
                let bytes = fs::read(&path)
                    .map_err(|error| CasError::io("read snapshot file", &path, error))?;
                (
                    EntryKind::File,
                    self.cas.put(&bytes)?,
                    is_executable(&metadata),
                )
            } else if file_type.is_dir() {
                (EntryKind::Directory, self.build_directory(&path)?, false)
            } else {
                // This explicitly includes symlinks; they are never followed.
                return Err(CasError::UnsupportedEntry(path));
            };
            children.push(MerkleEntry {
                name,
                kind,
                digest,
                executable,
            });
        }
        children.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
        reject_case_collisions(directory, &children)?;
        let tree = MerkleTree {
            schema: TREE_SCHEMA.into(),
            entries: children,
        };
        let encoded = serde_json::to_vec(&tree)
            .map_err(|error| CasError::Serialization(error.to_string()))?;
        self.cas.put(&encoded)
    }
}

pub fn build_snapshot(cas: &LocalCas, root: impl AsRef<Path>) -> Result<Digest, CasError> {
    SnapshotBuilder::new(cas).build(root)
}

pub fn materialize_snapshot(
    cas: &LocalCas,
    root: Digest,
    destination: impl AsRef<Path>,
) -> Result<(), CasError> {
    let destination = destination.as_ref();
    ensure_safe_destination(destination)?;
    let mut ancestors = HashSet::new();
    materialize_tree(cas, root, destination, &mut ancestors)
}

fn materialize_tree(
    cas: &LocalCas,
    digest: Digest,
    destination: &Path,
    ancestors: &mut HashSet<Digest>,
) -> Result<(), CasError> {
    if !ancestors.insert(digest) {
        return Err(CasError::InvalidSnapshot(format!(
            "directory cycle at {digest}"
        )));
    }
    let result = (|| {
        let tree = load_tree(cas, digest)?;
        validate_tree(destination, &tree)?;
        if !destination.exists() {
            fs::create_dir(destination).map_err(|error| {
                CasError::io("create materialized directory", destination, error)
            })?;
        }
        for entry in tree.entries {
            let path = destination.join(&entry.name);
            match entry.kind {
                EntryKind::Directory => materialize_tree(cas, entry.digest, &path, ancestors)?,
                EntryKind::File => materialize_file(cas, entry.digest, &path, entry.executable)?,
            }
        }
        Ok(())
    })();
    ancestors.remove(&digest);
    result
}

fn materialize_file(
    cas: &LocalCas,
    digest: Digest,
    path: &Path,
    executable: bool,
) -> Result<(), CasError> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(CasError::InvalidPath(format!(
            "destination already contains {}",
            path.display()
        )));
    }
    let bytes = cas.get(digest)?;
    let parent = path
        .parent()
        .ok_or_else(|| CasError::InvalidPath("file destination has no parent".into()))?;
    let temporary = parent.join(format!(
        ".jeden-materialize-{}-{}",
        std::process::id(),
        digest
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| CasError::io("create materialized temporary file", &temporary, error))?;
    let write_result = (|| {
        file.write_all(&bytes)
            .map_err(|error| CasError::io("write materialized file", &temporary, error))?;
        file.sync_all()
            .map_err(|error| CasError::io("sync materialized file", &temporary, error))?;
        set_executable(&file, executable, &temporary)?;
        drop(file);
        fs::rename(&temporary, path)
            .map_err(|error| CasError::io("commit materialized file", path, error))?;
        super::store::sync_directory(parent)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}


pub(crate) fn load_tree(cas: &LocalCas, digest: Digest) -> Result<MerkleTree, CasError> {
    let bytes = cas.get(digest)?;
    let tree: MerkleTree = serde_json::from_slice(&bytes).map_err(|error| {
        CasError::InvalidSnapshot(format!("object {digest} is not a tree: {error}"))
    })?;
    if tree.schema != TREE_SCHEMA {
        return Err(CasError::InvalidSnapshot(format!(
            "object {digest} is not a supported tree"
        )));
    }
    Ok(tree)
}

pub(crate) fn parse_tree(bytes: &[u8]) -> Option<MerkleTree> {
    let tree: MerkleTree = serde_json::from_slice(bytes).ok()?;
    (tree.schema == TREE_SCHEMA).then_some(tree)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_executable(file: &std::fs::File, executable: bool, path: &Path) -> Result<(), CasError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|error| CasError::io("set materialized permissions", path, error))
}
#[cfg(not(unix))]
fn set_executable(_: &std::fs::File, _: bool, _: &Path) -> Result<(), CasError> {
    Ok(())
}
