use super::manifest::PluginReleaseV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
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
        let mut bytes = serde_json::to_vec(&copy)
            .map_err(|error| format!("the plugin lock cannot be encoded as JSON: {error}"))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// The lock's canonical bytes over `path` through the shared durable
    /// replace; encoded before anything is staged, so a lock that cannot be
    /// encoded leaves nothing behind.
    pub fn write_atomic(&self, path: &Path) -> Result<(), String> {
        let bytes = self.canonical_bytes()?;
        crate::tool_runtime::runtime_ops::durable::replace(path, &bytes)
    }
}
