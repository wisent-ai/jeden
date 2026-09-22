//! Remembering which subscription pools are resting after a quota refusal,
//! across restarts.
//!
//! Split out of `routing/store.rs`, which had grown past the module line cap.

use super::super::SubscriptionTargetIdentity;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CooldownEntry {
    target: SubscriptionTargetIdentity,
    until_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CooldownDocument {
    version: u32,
    entries: Vec<CooldownEntry>,
}

/// Durable quota cooldowns. An entry whose moment has passed is ignored on
/// read; recording reads the document on disk first and keeps whichever expiry
/// is later, so two clones recording at once cannot shorten a rest.
#[derive(Clone)]
pub struct CooldownStore {
    path: PathBuf,
    operation: Arc<Mutex<()>>,
}

impl CooldownStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let store = Self {
            path,
            operation: Arc::new(Mutex::new(())),
        };
        store.load()?;
        Ok(store)
    }

    pub fn is_cooling_down(
        &self,
        target: &SubscriptionTargetIdentity,
        now_ms: u64,
    ) -> Result<bool, String> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| "cooldown store poisoned")?;
        Ok(self
            .load()?
            .entries
            .iter()
            .any(|entry| &entry.target == target && entry.until_ms > now_ms))
    }

    pub fn cooldown_until(
        &self,
        target: &SubscriptionTargetIdentity,
    ) -> Result<Option<u64>, String> {
        let _guard = self
            .operation
            .lock()
            .map_err(|_| "cooldown store poisoned")?;
        Ok(self
            .load()?
            .entries
            .iter()
            .find(|entry| &entry.target == target)
            .map(|entry| entry.until_ms))
    }

    pub fn record(
        &self,
        target: SubscriptionTargetIdentity,
        until_ms: u64,
        now_ms: u64,
    ) -> Result<(), String> {
        if until_ms <= now_ms {
            return Err("cooldown deadline must be in the future".into());
        }
        let _guard = self
            .operation
            .lock()
            .map_err(|_| "cooldown store poisoned")?;
        let mut document = self.load()?;
        document.entries.retain(|entry| entry.until_ms > now_ms);
        if let Some(entry) = document
            .entries
            .iter_mut()
            .find(|entry| entry.target == target)
        {
            entry.until_ms = entry.until_ms.max(until_ms);
        } else {
            document.entries.push(CooldownEntry { target, until_ms });
            document
                .entries
                .sort_by(|left, right| left.target.cmp(&right.target));
        }
        self.persist(&document)
    }

    fn load(&self) -> Result<CooldownDocument, String> {
        if !self.path.exists() {
            return Ok(CooldownDocument {
                version: 1,
                entries: Vec::new(),
            });
        }
        let document: CooldownDocument =
            serde_json::from_slice(&fs::read(&self.path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if document.version != 1 {
            return Err(format!(
                "unsupported cooldown store version {}",
                document.version
            ));
        }
        Ok(document)
    }

    fn persist(&self, document: &CooldownDocument) -> Result<(), String> {
        let parent = self.path.parent().ok_or("cooldown path has no parent")?;
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let _ = fs::remove_file(&temporary);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let result = (|| {
            serde_json::to_writer(&mut file, document).map_err(|error| error.to_string())?;
            file.write_all(b"\n").map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, &self.path).map_err(|error| error.to_string())?;
            OpenOptions::new()
                .read(true)
                .open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
