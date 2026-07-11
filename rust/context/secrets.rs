use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::cli::config::{SecretMode, SecretsConfig};

use super::discovery::project_root;

#[derive(Debug, Clone)]
pub(crate) struct SecretSource {
    pub(crate) provenance: String,
    pub(crate) value: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SecretPolicy {
    mode: SecretMode,
    replacement: String,
    secrets: Vec<SecretSource>,
}

impl SecretPolicy {
    pub(crate) fn load(cwd: &Path, config: &SecretsConfig) -> Result<Self, String> {
        let root = project_root(cwd)?;
        let mut sources = Vec::new();
        for (index, value) in config.values.iter().enumerate() {
            add_secret(
                &mut sources,
                format!("config:secrets.values[{index}]"),
                value,
                1,
            );
        }
        for name in &config.environment {
            let value = env::var(name)
                .map_err(|_| format!("configured secret environment variable {name} is not set"))?;
            add_secret(
                &mut sources,
                format!("environment:{name}"),
                &value,
                1,
            );
        }
        if config.discover_environment {
            for (name, value) in env::vars() {
                if looks_secret_name(&name) {
                    add_secret(
                        &mut sources,
                        format!("environment:{name}"),
                        &value,
                        config.min_length,
                    );
                }
            }
        }
        for configured in &config.files {
            let requested = root.join(configured);
            let canonical = requested.canonicalize().map_err(|error| {
                format!("configured secret file {} cannot be resolved: {error}", requested.display())
            })?;
            if !canonical.starts_with(&root) {
                return Err(format!(
                    "configured secret file {} escapes path jail {}",
                    canonical.display(),
                    root.display()
                ));
            }
            load_secret_file(&canonical, 1, &mut sources)?;
        }
        let mut seen = BTreeSet::new();
        sources.retain(|source| seen.insert(source.value.clone()));
        sources.sort_by(|left, right| right.value.len().cmp(&left.value.len()));
        Ok(Self {
            mode: config.mode,
            replacement: config.replacement.clone(),
            secrets: sources,
        })
    }

    pub(crate) fn provenance(&self) -> Vec<&str> {
        self.secrets
            .iter()
            .map(|source| source.provenance.as_str())
            .collect()
    }

    pub(crate) fn protect_text(&self, text: &str) -> String {
        let mut protected = text.to_string();
        for source in &self.secrets {
            if protected.contains(&source.value) {
                let replacement = match self.mode {
                    SecretMode::Redact => self.replacement.clone(),
                    SecretMode::Obfuscate => obfuscate(&source.value),
                };
                protected = protected.replace(&source.value, &replacement);
            }
        }
        protected
    }

    /// Produces a model-bound copy. The local value remains untouched, so local
    /// transcripts can retain context provenance while the provider sees no
    /// configured or automatically discovered secret values.
    pub(crate) fn protect_json(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.protect_text(text)),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.protect_json(item)).collect())
            }
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (self.protect_text(key), self.protect_json(value)))
                    .collect(),
            ),
            primitive => primitive.clone(),
        }
    }

    pub(crate) fn protect_messages(&self, messages: &[Value]) -> Vec<Value> {
        messages.iter().map(|message| self.protect_json(message)).collect()
    }
}

fn add_secret(sources: &mut Vec<SecretSource>, provenance: String, value: &str, min_length: usize) {
    if !value.is_empty() && value.len() >= min_length {
        sources.push(SecretSource {
            provenance,
            value: value.to_string(),
        });
    }
}

fn looks_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "PRIVATE_KEY", "ACCESS_KEY"]
        .iter()
        .any(|marker| upper.contains(marker))
}

fn load_secret_file(
    path: &Path,
    min_length: usize,
    sources: &mut Vec<SecretSource>,
) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read secret file {}: {error}", path.display()))?;
    for (line_index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let value = trimmed
            .split_once('=')
            .map(|(_, value)| value.trim().trim_matches(['\'', '"']))
            .unwrap_or(trimmed);
        add_secret(
            sources,
            format!("file:{}:{}", path.display(), line_index + 1),
            value,
            min_length,
        );
    }
    Ok(())
}

fn obfuscate(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 4 {
        return "*".repeat(chars.len());
    }
    format!(
        "{}{}{}",
        chars[0],
        "*".repeat(chars.len().saturating_sub(2)),
        chars[chars.len() - 1]
    )
}

