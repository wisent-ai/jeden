mod storage;

use super::lock::PluginLockV1;
use super::manifest::{MarketplaceEnvelopeV1, PluginDependency};
use super::resolver;
use super::trust::TrustRootV1;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    Installed,
    Active,
    Quarantined,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRecord {
    pub id: String,
    pub version: String,
    pub digest: String,
    pub path: PathBuf,
    pub entrypoint: String,
    pub state: PackageState,
    pub trust: String,
    pub generation: u64,
    #[serde(default)]
    pub quarantine_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRegistryV1 {
    pub generation: u64,
    pub catalog_sequence: u64,
    pub packages: BTreeMap<String, PackageRecord>,
}

pub struct MarketplaceService {
    root: PathBuf,
}
impl MarketplaceService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn install_and_activate<F>(
        &self,
        root: &TrustRootV1,
        envelope: &MarketplaceEnvelopeV1,
        previous_sequence: Option<u64>,
        now: u64,
        requested: &[PluginDependency],
        platform: &str,
        mut fetch: F,
    ) -> Result<ActiveRegistryV1, String>
    where
        F: FnMut(&str) -> Result<Vec<u8>, String>,
    {
        root.verify_catalog(envelope, now, previous_sequence)?;
        let resolved = resolver::resolve(requested, &envelope.catalog.releases, platform)?;
        let lock = PluginLockV1::from_resolution(
            &envelope.catalog.catalog_id,
            envelope.catalog.sequence,
            &envelope.signing_bytes()?,
            platform,
            &resolved,
        );
        let transaction = self.root.join(format!(
            "transaction-{}-{}",
            std::process::id(),
            envelope.catalog.sequence
        ));
        if transaction.exists() {
            fs::remove_dir_all(&transaction).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(&transaction).map_err(|error| error.to_string())?;
        let mut promoted = Vec::<PathBuf>::new();
        let mut backups = Vec::<(PathBuf, PathBuf)>::new();
        let result = (|| {
            let current = self.registry()?;
            let next_generation = current.generation.saturating_add(1);
            let mut next = current.clone();
            next.generation = next_generation;
            next.catalog_sequence = envelope.catalog.sequence;
            for release in &resolved {
                if envelope
                    .catalog
                    .revoked_artifacts
                    .contains(&release.artifact_digest)
                {
                    return Err(format!("artifact for {} is revoked", release.id));
                }
                let bytes = fetch(&release.artifact_url)?;
                let digest =
                    self.store_verified(&release.artifact_digest, release.artifact_size, &bytes)?;
                let staged = transaction.join("staged").join(&release.id);
                self.unpack_verified(digest, &staged)?;
                if !staged.join(&release.entrypoint).is_file() {
                    return Err(format!(
                        "plugin {} initialization failed: entrypoint missing",
                        release.id
                    ));
                }
                let final_path = self.packages().join(format!(
                    "{}-{}-{}",
                    release.id,
                    release.version,
                    &release.artifact_digest[..16]
                ));
                next.packages.insert(
                    release.id.clone(),
                    PackageRecord {
                        id: release.id.clone(),
                        version: release.version.clone(),
                        digest: release.artifact_digest.clone(),
                        path: final_path,
                        entrypoint: release.entrypoint.clone(),
                        state: PackageState::Installed,
                        trust: format!(
                            "catalog:{}:{}",
                            envelope.catalog.catalog_id, envelope.catalog.sequence
                        ),
                        generation: next_generation,
                        quarantine_reason: None,
                    },
                );
            }
            fs::create_dir_all(self.packages()).map_err(|error| error.to_string())?;
            for record in next
                .packages
                .values_mut()
                .filter(|record| record.generation == next_generation)
            {
                let staged = transaction.join("staged").join(&record.id);
                if record.path.exists() {
                    let backup = transaction.join("backups").join(&record.id);
                    if let Some(parent) = backup.parent() {
                        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    fs::rename(&record.path, &backup).map_err(|error| error.to_string())?;
                    backups.push((backup, record.path.clone()));
                }
                fs::rename(&staged, &record.path).map_err(|error| error.to_string())?;
                promoted.push(record.path.clone());
                record.state = PackageState::Active;
            }
            lock.write_atomic(&self.lock_path())?;
            self.atomic_json(&self.registry_path(), &next)?;
            crate::capability::invalidate();
            Ok(next)
        })();
        if result.is_err() {
            for path in promoted.iter().rev() {
                let _ = fs::remove_dir_all(path);
            }
            for (backup, original) in backups.iter().rev() {
                let _ = fs::rename(backup, original);
            }
        }
        let _ = fs::remove_dir_all(&transaction);
        result
    }

    pub fn dev_link(
        &self,
        id: &str,
        path: &Path,
        entrypoint: &str,
    ) -> Result<PackageRecord, String> {
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.is_dir() || !canonical.join(entrypoint).is_file() {
            return Err("dev-link requires an existing local directory and entrypoint".into());
        }
        let mut registry = self.registry()?;
        registry.generation = registry.generation.saturating_add(1);
        let record = PackageRecord {
            id: id.into(),
            version: "dev-link".into(),
            digest: String::new(),
            path: canonical,
            entrypoint: entrypoint.into(),
            state: PackageState::Installed,
            trust: "dev-link:explicit-local-untrusted".into(),
            generation: registry.generation,
            quarantine_reason: None,
        };
        registry.packages.insert(id.into(), record.clone());
        self.atomic_json(&self.registry_path(), &registry)?;
        Ok(record)
    }

    pub fn quarantine_revoked(&self, revoked: &BTreeSet<String>) -> Result<Vec<String>, String> {
        let mut registry = self.registry()?;
        let mut quarantined = Vec::new();
        for record in registry.packages.values_mut() {
            if revoked.contains(&record.digest) && record.state == PackageState::Active {
                record.state = PackageState::Quarantined;
                record.quarantine_reason = Some("artifact revoked by verified catalog".into());
                quarantined.push(record.id.clone());
            }
        }
        if !quarantined.is_empty() {
            registry.generation = registry.generation.saturating_add(1);
            self.atomic_json(&self.registry_path(), &registry)?;
            crate::capability::invalidate();
        }
        quarantined.sort();
        Ok(quarantined)
    }

    pub fn active_packages(&self) -> Result<Vec<PackageRecord>, String> {
        let mut active = self
            .registry()?
            .packages
            .into_values()
            .filter(|record| record.state == PackageState::Active)
            .collect::<Vec<_>>();
        active.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(active)
    }
}
