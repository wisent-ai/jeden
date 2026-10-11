use crate::cli::config::config_layer_paths;
use serde_json::Value;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

const SERVICE_ENVS: &[&str] = &[
    "JEDEN_BROWSER_BRIDGE",
    "JEDEN_BROWSER_MODE",
    "JEDEN_BROWSER_PROFILE",
    "JEDEN_CHROME_EXECUTABLE",
    "JEDEN_DAP_ADAPTER",
    "TAVILY_API_KEY",
    "BRAVE_SEARCH_API_KEY",
    "JEDEN_TAVILY_URL",
    "JEDEN_BRAVE_SEARCH_URL",
    "JEDEN_IMAGE_MODEL",
    "JEDEN_TTS_MODEL",
];

pub(crate) fn fingerprint(cwd: &Path) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for path in config_layer_paths(cwd) {
        path.hash(&mut hash);
        if let Ok(metadata) = fs::metadata(&path) {
            metadata.len().hash(&mut hash);
            metadata.modified().ok().hash(&mut hash);
        }
    }
    for name in SERVICE_ENVS {
        name.hash(&mut hash);
        std::env::var_os(name).hash(&mut hash);
    }
    hash.finish()
}

pub(crate) fn string(config: &Value, path: &[&str], env: &str) -> Option<String> {
    std::env::var(env)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            let mut value = config;
            for key in path {
                value = value.get(*key)?;
            }
            value
                .as_str()
                .map(str::to_owned)
                .filter(|v| !v.trim().is_empty())
        })
}

pub(crate) fn strings(config: &Value, path: &[&str]) -> Vec<String> {
    let mut value = config;
    for key in path {
        let Some(next) = value.get(*key) else {
            return Vec::new();
        };
        value = next;
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
