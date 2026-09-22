//! Getting a package's bytes onto disk without trusting them.
//!
//! Split out of `marketplace/service.rs`, which had grown past the module
//! line cap.

use super::{ActiveRegistryV1, MarketplaceService};
use crate::cas::{Digest, LocalCas};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Component, Path, PathBuf};

impl MarketplaceService {
    pub(super) fn cas(&self) -> Result<LocalCas, String> {
        let jeden_root = self
            .root
            .parent()
            .and_then(Path::parent)
            .ok_or("marketplace root must be nested under .jeden/plugins")?;
        LocalCas::open(jeden_root.join("cas")).map_err(|error| error.to_string())
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
        if !path.exists() {
            return Ok(ActiveRegistryV1::default());
        }
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())
    }

    pub(super) fn atomic_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), String> {
        let parent = path.parent().ok_or("registry path has no parent")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        let result = (|| {
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
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
            .map_err(|error| error.to_string())?;
        self.cas()?
            .put_verified(expected, bytes)
            .map_err(|error| error.to_string())?;
        Ok(expected)
    }

    pub(super) fn unpack_verified(&self, digest: Digest, destination: &Path) -> Result<(), String> {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        let mut archive = tar::Archive::new(Cursor::new(
            self.cas()?.get(digest).map_err(|error| error.to_string())?,
        ));
        for item in archive.entries().map_err(|error| error.to_string())? {
            let mut item = item.map_err(|error| error.to_string())?;
            let relative = item.path().map_err(|error| error.to_string())?.into_owned();
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
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
                continue;
            }
            if !kind.is_file() {
                return Err("plugin artifact contains unsupported entry type".into());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            item.unpack(&target).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    // Trust root, signed envelope, replay bound, clock, requested deps, target
    // platform, and fetcher each come from a different caller, so grouping them
    // would only move the same argument list one call up.
}
