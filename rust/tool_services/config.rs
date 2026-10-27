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

/// A key that changes whenever a config layer or a service variable changes. A layer's
/// size and modification time enter it, or the kind of error that kept them from being
/// read, so a layer that becomes unreadable changes the key too.
pub(crate) fn fingerprint(cwd: &Path) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for path in config_layer_paths(cwd) {
        path.hash(&mut hash);
        match fs::metadata(&path) {
            Ok(metadata) => {
                metadata.len().hash(&mut hash);
                metadata.modified().map_err(|error| error.kind()).hash(&mut hash);
            }
            Err(error) => error.kind().hash(&mut hash),
        }
    }
    for name in SERVICE_ENVS {
        name.hash(&mut hash);
        std::env::var_os(name).hash(&mut hash);
    }
    hash.finish()
}

/// A service setting: the variable `env` when it holds text, else the string at `path`
/// in the merged config; `None` when neither states one. A variable that is set but not
/// Unicode cannot be used; that is said on stderr, naming it, before the config is read.
pub(crate) fn string(config: &Value, path: &[&str], env: &str) -> Option<String> {
    match std::env::var(env) {
        Ok(value) if !value.trim().is_empty() => return Some(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => {}
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("jeden: {env} is not Unicode; reading {} from the config", path.join("."));
        }
    }
    let mut value = config;
    for key in path {
        value = value.get(*key)?;
    }
    value
        .as_str()
        .map(str::to_owned)
        .filter(|v| !v.trim().is_empty())
}

/// The list of text at `path` in the merged config, read by the shared
/// `config_texts`: none when the config states none, and a value in another
/// shape refused naming the setting.
pub(crate) fn strings(config: &Value, path: &[&str]) -> Result<Vec<String>, String> {
    let setting = path.join(".");
    match crate::cli::config::config_value_at(config, &setting) {
        Some(value) => Ok(crate::cli::config::config_texts(value, &setting)?
            .into_iter()
            .map(str::to_owned)
            .collect()),
        None => Ok(Vec::new()),
    }
}
