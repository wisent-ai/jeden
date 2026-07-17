pub mod billing;
pub mod brama;
pub mod contract;
pub mod staging;
pub mod transport;
pub mod weles;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealth {
    pub service: String,
    pub version: String,
    pub available: bool,
    pub endpoint: Option<String>,
    pub detail: String,
    pub checked_at_ms: u64,
}

pub fn model_catalog(
    cwd: &Path,
    client: &brama::BramaClient,
    force: bool,
) -> Result<brama::ModelCatalog, brama::BramaError> {
    let mut catalog = client.catalog(force)?;
    let mut entries = catalog
        .models
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in crate::hooks::extension_model_entries(cwd) {
        entries.insert(entry.id.clone(), entry);
    }
    catalog.models = entries.into_values().collect();
    Ok(catalog)
}

pub fn providers(
    cwd: &Path,
    client: &weles::WelesClient,
) -> Result<Vec<weles::Provider>, weles::WelesError> {
    let mut entries = client
        .providers()?
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in crate::hooks::extension_provider_entries(cwd) {
        entries.insert(entry.id.clone(), entry);
    }
    Ok(entries.into_values().collect())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
