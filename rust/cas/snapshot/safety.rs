//! Everything a snapshot must satisfy before a byte is written to the
//! machine, and everything a tree must satisfy before it is trusted.
//!
//! Split out of `cas/snapshot.rs`, which had grown past the module line cap.

use super::{EntryKind, MerkleEntry, MerkleTree, TREE_SCHEMA};
use crate::cas::CasError;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path};

pub(super) fn ensure_safe_destination(path: &Path) -> Result<(), CasError> {
    if path.as_os_str().is_empty() {
        return Err(CasError::InvalidPath("empty destination".into()));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_dir() {
            return Err(CasError::UnsupportedEntry(path.to_path_buf()));
        }
        let mut entries = fs::read_dir(path)
            .map_err(|error| CasError::io("inspect materialization destination", path, error))?;
        if entries.next().is_some() {
            return Err(CasError::InvalidPath(format!(
                "destination {} is not empty",
                path.display()
            )));
        }
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| CasError::InvalidPath("destination has no parent".into()))?;
        verify_existing_ancestors(parent)?;
    }
    Ok(())
}

fn verify_existing_ancestors(path: &Path) -> Result<(), CasError> {
    let mut existing = path;
    while !existing.as_os_str().is_empty() {
        match fs::symlink_metadata(existing) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() {
                    return Err(CasError::UnsupportedEntry(existing.to_path_buf()));
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| {
                    CasError::InvalidPath("destination has no existing ancestor".into())
                })?;
            }
            Err(error) => {
                return Err(CasError::io(
                    "inspect destination ancestor",
                    existing,
                    error,
                ))
            }
        }
    }
    Ok(())
}

pub(super) fn validate_tree(directory: &Path, tree: &MerkleTree) -> Result<(), CasError> {
    if tree.schema != TREE_SCHEMA {
        return Err(CasError::InvalidSnapshot(format!(
            "unsupported tree schema {:?}",
            tree.schema
        )));
    }
    let mut previous: Option<&str> = None;
    for entry in &tree.entries {
        validate_component(&entry.name)?;
        if let Some(name) = previous {
            if name.as_bytes() >= entry.name.as_bytes() {
                return Err(CasError::InvalidSnapshot(
                    "tree entries are not in strict deterministic order".into(),
                ));
            }
        }
        if entry.kind == EntryKind::Directory && entry.executable {
            return Err(CasError::InvalidSnapshot(format!(
                "directory {:?} has a file executable flag",
                entry.name
            )));
        }
        previous = Some(&entry.name);
    }
    reject_case_collisions(directory, &tree.entries)
}

pub(crate) fn reject_case_collisions(directory: &Path, entries: &[MerkleEntry]) -> Result<(), CasError> {
    let mut folded = BTreeMap::<String, &str>::new();
    for entry in entries {
        let key = unicode_case_fold(&entry.name);
        if let Some(first) = folded.insert(key, &entry.name) {
            return Err(CasError::CaseCollision {
                directory: directory.to_path_buf(),
                first: first.into(),
                second: entry.name.clone(),
            });
        }
    }
    Ok(())
}

fn unicode_case_fold(value: &str) -> String {
    // Upper-then-lower folding handles context-sensitive forms such as Greek
    // final sigma. Repeating once also expands forms such as capital sharp S.
    let once: String = value
        .chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect();
    once.chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn validate_component(name: &str) -> Result<(), CasError> {
    if name.is_empty() {
        return Err(CasError::InvalidPath("empty component".into()));
    }
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || path.as_os_str() == OsStr::new(".")
        || path.as_os_str() == OsStr::new("..")
    {
        return Err(CasError::InvalidPath(format!(
            "{name:?} is not one normal path component"
        )));
    }
    Ok(())
}
