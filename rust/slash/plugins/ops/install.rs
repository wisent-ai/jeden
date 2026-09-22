//! Installing one plugin from a signed catalogue, and refusing anything less.
//!
//! Split out of `slash/plugins/ops.rs`, which had grown past the module line
//! cap.

use super::super::fetch::{fetch_marketplace, read_marketplace_catalog};
use super::super::production::manifest::{MarketplaceEnvelopeV1, PluginDependency};
use super::super::production::trust::TrustRootV1;
use super::super::marketplace_cache_dir;
use super::{production_service, registry_scope_dir};
use crate::slash::validate::{valid_marketplace_name, valid_plugin_id, valid_plugin_name};
use std::fs;
use std::path::Path;
use crate::slash::plugins::ops::find_marketplace_source;

fn artifact_bytes(cache: &Path, location: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = location.strip_prefix("file://") {
        return fs::read(path).map_err(|error| error.to_string());
    }
    if location.starts_with("https://") {
        let response = reqwest::blocking::get(location).map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "artifact download failed with {}",
                response.status()
            ));
        }
        return response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|error| error.to_string());
    }
    let path = cache.join(location);
    let canonical_cache = cache.canonicalize().map_err(|error| error.to_string())?;
    let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical_path.starts_with(canonical_cache) {
        return Err("artifact path escapes verified catalog cache".into());
    }
    fs::read(canonical_path).map_err(|error| error.to_string())
}

/// Resolve and transactionally activate a signed production catalog. Unsigned
/// legacy catalogs are deliberately rejected; local development uses dev-link.
pub(crate) fn install_one(
    cwd: &Path,
    mkt_name: &str,
    plugin_name: &str,
    scope: &str,
    force: bool,
) -> Result<String, String> {
    if !valid_marketplace_name(mkt_name) {
        return Err(format!("Invalid marketplace name: {mkt_name}"));
    }
    if !valid_plugin_name(plugin_name) {
        return Err(format!("Invalid plugin name: {plugin_name}"));
    }
    let id = format!("{plugin_name}@{mkt_name}");
    if !valid_plugin_id(&id) {
        return Err(format!("Invalid plugin id: {id}"));
    }
    let source = find_marketplace_source(cwd, mkt_name).ok_or_else(|| {
        format!("Marketplace source not found: {mkt_name}. Add a signed source first.")
    })?;
    let cache = marketplace_cache_dir(mkt_name);
    if !cache.exists() {
        fetch_marketplace(cwd, mkt_name, &source)?;
    }
    let envelope: MarketplaceEnvelopeV1 = serde_json::from_value(read_marketplace_catalog(&cache)?).map_err(|error| format!("marketplace catalog is not a signed MarketplaceEnvelopeV1: {error}; use explicit dev-link for local development"))?;
    let scope_dir = registry_scope_dir(cwd, scope);
    let trust_path = scope_dir.join(".jeden/marketplace-trust-root.json");
    let trust: TrustRootV1 = serde_json::from_slice(&fs::read(&trust_path).map_err(|error| {
        format!(
            "cannot load marketplace trust root {}: {error}",
            trust_path.display()
        )
    })?)
    .map_err(|error| format!("invalid marketplace trust root: {error}"))?;
    let service = production_service(&scope_dir);
    if !force
        && service
            .active_packages()?
            .iter()
            .any(|record| record.id == plugin_name)
    {
        return Err(format!(
            "{id} is already active; use --force to replace it transactionally"
        ));
    }
    let requested = [PluginDependency {
        id: plugin_name.into(),
        requirement: "*".into(),
        features: Default::default(),
        optional: false,
    }];
    let previous = service
        .registry()?
        .catalog_sequence
        .checked_sub(0)
        .filter(|sequence| *sequence > 0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let active = service.install_and_activate(
        &trust,
        &envelope,
        previous,
        now,
        &requested,
        std::env::consts::OS,
        |location| artifact_bytes(&cache, location),
    )?;
    let record = active
        .packages
        .get(plugin_name)
        .ok_or_else(|| format!("resolved activation omitted requested plugin {plugin_name}"))?;
    Ok(format!(
        "activated signed {id} version {} at generation {} [scope: {scope}, digest {}]",
        record.version, active.generation, record.digest
    ))
}
