use super::manifest::PluginReleaseV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedPluginV1 {
    pub id: String,
    pub version: String,
    pub artifact_digest: String,
    pub artifact_size: u64,
    pub entrypoint: String,
    pub features: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginLockV1 {
    pub schema: String,
    pub catalog_id: String,
    pub catalog_sequence: u64,
    pub catalog_digest: String,
    pub platform: String,
    pub plugins: Vec<LockedPluginV1>,
}

impl PluginLockV1 {
    pub fn from_resolution(
        catalog_id: impl Into<String>,
        sequence: u64,
        catalog_bytes: &[u8],
        platform: impl Into<String>,
        releases: &[&PluginReleaseV1],
    ) -> Self {
        let mut plugins = releases
            .iter()
            .map(|release| LockedPluginV1 {
                id: release.id.clone(),
                version: release.version.clone(),
                artifact_digest: release.artifact_digest.to_ascii_lowercase(),
                artifact_size: release.artifact_size,
                entrypoint: release.entrypoint.clone(),
                features: release.features.clone(),
            })
            .collect::<Vec<_>>();
        plugins.sort();
        Self {
            schema: "jeden.plugin-lock.v1".into(),
            catalog_id: catalog_id.into(),
            catalog_sequence: sequence,
            catalog_digest: hex::encode(Sha256::digest(catalog_bytes)),
            platform: platform.into(),
            plugins,
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        let mut copy = self.clone();
        copy.plugins.sort();
        let mut bytes = serde_json::to_vec(&copy).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn write_atomic(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or("lock path has no parent")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let result = (|| {
            file.write_all(&self.canonical_bytes()?)
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
