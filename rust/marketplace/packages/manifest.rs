use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MARKETPLACE_SCHEMA: &str = "jeden.marketplace.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvelopeSignature {
    pub key_id: String,
    pub signature: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDependency {
    pub id: String,
    pub requirement: String,
    #[serde(default)]
    pub features: BTreeSet<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginReleaseV1 {
    pub id: String,
    pub version: String,
    pub artifact_digest: String,
    pub artifact_size: u64,
    pub artifact_url: String,
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    #[serde(default)]
    pub features: BTreeSet<String>,
    #[serde(default)]
    pub platforms: BTreeSet<String>,
    pub entrypoint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceCatalogV1 {
    pub catalog_id: String,
    pub sequence: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub releases: Vec<PluginReleaseV1>,
    #[serde(default)]
    pub revoked_keys: BTreeSet<String>,
    #[serde(default)]
    pub revoked_artifacts: BTreeSet<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceEnvelopeV1 {
    pub schema: String,
    pub root_version: u64,
    pub catalog: MarketplaceCatalogV1,
    pub signatures: Vec<EnvelopeSignature>,
}

impl MarketplaceEnvelopeV1 {
    pub fn signing_bytes(&self) -> Result<Vec<u8>, String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Signed<'a> {
            schema: &'a str,
            root_version: u64,
            catalog: &'a MarketplaceCatalogV1,
        }
        serde_json::to_vec(&Signed {
            schema: &self.schema,
            root_version: self.root_version,
            catalog: &self.catalog,
        })
        .map_err(|error| error.to_string())
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema != MARKETPLACE_SCHEMA {
            return Err(format!("unsupported marketplace schema {}", self.schema));
        }
        if self.catalog.catalog_id.trim().is_empty() {
            return Err("catalog ID is empty".into());
        }
        for release in &self.catalog.releases {
            if release.id.trim().is_empty() || release.version.trim().is_empty() {
                return Err("release ID/version is empty".into());
            }
            if release.artifact_digest.len() != 64
                || !release
                    .artifact_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!("release {} has invalid SHA-256 digest", release.id));
            }
            if release.entrypoint.is_empty()
                || release.entrypoint.starts_with('/')
                || release
                    .entrypoint
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(format!("release {} has unsafe entrypoint", release.id));
            }
        }
        Ok(())
    }
}
