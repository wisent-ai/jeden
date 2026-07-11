pub(crate) mod brama;
pub(crate) mod weles;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceHealth {
    pub service: String,
    pub version: String,
    pub available: bool,
    pub endpoint: Option<String>,
    pub detail: String,
    pub checked_at_ms: u64,
}

pub(crate) fn model_catalog(cwd: &Path, client: &brama::BramaClient, force: bool) -> Result<brama::ModelCatalog, brama::BramaError> {
    let mut catalog = client.catalog(force)?;
    let mut entries = catalog.models.into_iter().map(|entry| (entry.id.clone(), entry)).collect::<BTreeMap<_, _>>();
    for entry in crate::hooks::extension_model_entries(cwd) { entries.insert(entry.id.clone(), entry); }
    catalog.models = entries.into_values().collect();
    Ok(catalog)
}

pub(crate) fn providers(cwd: &Path, client: &weles::WelesClient) -> Result<Vec<weles::Provider>, weles::WelesError> {
    let mut entries = client.providers()?.into_iter().map(|entry| (entry.id.clone(), entry)).collect::<BTreeMap<_, _>>();
    for entry in crate::hooks::extension_provider_entries(cwd) { entries.insert(entry.id.clone(), entry); }
    Ok(entries.into_values().collect())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
