//! Getting a package's bytes onto disk without trusting them.
//!
//! Split out of `marketplace/service.rs`, which had grown past the module
//! line cap.

use super::{ActiveRegistryV1, MarketplaceService};
use crate::cas::{Digest, LocalCas};
use serde::Serialize;
use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

impl MarketplaceService {
    pub(super) fn cas(&self) -> Result<LocalCas, String> {
        let jeden_root = self
            .root
            .parent()
            .and_then(Path::parent)
            .ok_or("marketplace root must be nested under .jeden/plugins")?;
        let cas = jeden_root.join("cas");
        LocalCas::open(&cas)
            .map_err(|error| format!("the content store {} cannot be opened: {error}", cas.display()))
    }
    pub(super) fn packages(&self) -> PathBuf {
        self.root.join("packages")
    }
    pub(super) fn registry_path(&self) -> PathBuf {
        self.root.join("active-registry.json")
    }
    pub(super) fn lock_path(&self) -> PathBuf {
        self.root.join("plugin.lock.json")
    }

    pub fn registry(&self) -> Result<ActiveRegistryV1, String> {
        let path = self.registry_path();
        // Only a registry that does not exist is empty: one that could not
        // be stat'ed was read as empty too, and the next install or removal
        // wrote that empty registry over every active plugin.
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ActiveRegistryV1::default())
            }
            Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
        };
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("{} is not a plugin registry: {error}", path.display()))
    }

    /// `value` as JSON over `path` through the shared durable replace, so a
    /// reader sees the previous registry or the new one. It is encoded
    /// before anything is staged, so a value that cannot be encoded leaves
    /// nothing behind.
    pub(super) fn atomic_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec(value)
            .map_err(|error| format!("{} cannot be encoded as JSON: {error}", path.display()))?;
        crate::tool_runtime::runtime_ops::durable::replace(path, &bytes)
    }

    pub(super) fn store_verified(
        &self,
        expected_digest: &str,
        expected_size: u64,
        bytes: &[u8],
    ) -> Result<Digest, String> {
        if bytes.len() as u64 != expected_size {
            return Err(format!(
                "artifact size mismatch: expected {expected_size}, got {}",
                bytes.len()
            ));
        }
        let expected = expected_digest
            .parse::<Digest>()
            .map_err(|error| format!("artifact digest {expected_digest} is not a digest: {error}"))?;
        self.cas()?
            .put_verified(expected, bytes)
            .map_err(|error| format!("artifact {expected_digest} cannot be stored: {error}"))?;
        Ok(expected)
    }

    pub(super) fn unpack_verified(&self, digest: Digest, destination: &Path) -> Result<(), String> {
        fs::create_dir_all(destination).map_err(|error| {
            format!("the plugin directory {} cannot be created: {error}", destination.display())
        })?;
        let stored = self
            .cas()?
            .get(digest)
            .map_err(|error| format!("artifact {digest} cannot be read from the content store: {error}"))?;
        let mut archive = tar::Archive::new(Cursor::new(stored));
        for item in archive
            .entries()
            .map_err(|error| format!("artifact {digest} is not a tar archive: {error}"))?
        {
            let mut item =
                item.map_err(|error| format!("an entry of artifact {digest} cannot be read: {error}"))?;
            let relative = item
                .path()
                .map_err(|error| format!("an entry path of artifact {digest} cannot be read: {error}"))?
                .into_owned();
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(format!("unsafe artifact path {}", relative.display()));
            }
            let kind = item.header().entry_type();
            if kind.is_symlink() || kind.is_hard_link() {
                return Err("plugin artifact links are prohibited".into());
            }
            let target = destination.join(relative);
            if kind.is_dir() {
                fs::create_dir_all(&target)
                    .map_err(|error| format!("{} cannot be created: {error}", target.display()))?;
                continue;
            }
            if !kind.is_file() {
                return Err("plugin artifact contains unsupported entry type".into());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("{} cannot be created: {error}", parent.display()))?;
            }
            item.unpack(&target)
                .map_err(|error| format!("{} cannot be unpacked: {error}", target.display()))?;
        }
        Ok(())
    }

    // Trust root, signed envelope, replay bound, clock, requested deps, target
    // platform, and fetcher each come from a different caller, so grouping them
    // would only move the same argument list one call up.
}
