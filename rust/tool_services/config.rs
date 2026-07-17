use super::types::config_path;
use serde_json::{Map, Value};
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
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "JEDEN_IMAGE_MODEL",
    "JEDEN_TTS_MODEL",
];

fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay,
    }
}

pub(crate) fn discover(cwd: &Path) -> Value {
    let mut value = Value::Object(Map::new());
    for path in config_path(cwd) {
        if let Ok(bytes) = fs::read(path) {
            if bytes.len() <= 2 * 1024 * 1024 {
                if let Ok(layer) = serde_json::from_slice(&bytes) {
                    merge(&mut value, layer);
                }
            }
        }
    }
    value
}

pub(crate) fn fingerprint(cwd: &Path) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for path in config_path(cwd) {
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
        .take(16)
        .collect()
}
