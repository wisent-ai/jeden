use super::types::{AgentDefinition, TaskError};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_DEFINITIONS: usize = 512;
const MAX_DEFINITION_BYTES: u64 = 256 * 1024;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn roots(cwd: &Path) -> Result<Vec<PathBuf>, TaskError> {
    let user = home().join(".jeden");
    let mut roots = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("agents"),
        user.join("agents"),
        cwd.join(".jeden/agents"),
    ];
    if let Ok(extension_roots) = crate::hooks::extension_agent_dirs(cwd) {
        roots.extend(extension_roots);
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    if out.len() >= MAX_DEFINITIONS || !root.exists() {
        return;
    }
    if root.is_file() {
        if matches!(
            root.extension().and_then(|v| v.to_str()),
            Some("json" | "yaml" | "yml")
        ) {
            out.push(root.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, out);
        } else {
            collect(&path, out);
        }
        if out.len() >= MAX_DEFINITIONS {
            break;
        }
    }
}

pub fn discover_agents(cwd: &Path) -> Result<Vec<AgentDefinition>, TaskError> {
    let mut files = Vec::new();
    for root in roots(cwd)? {
        collect(&root, &mut files);
    }
    let mut definitions = BTreeMap::new();
    for path in files {
        let metadata = fs::metadata(&path)?;
        if metadata.len() > MAX_DEFINITION_BYTES {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let parsed = match path.extension().and_then(|v| v.to_str()) {
            Some("yaml" | "yml") => serde_yaml::from_str::<AgentDefinition>(&text)
                .map_err(|e| TaskError::Invalid(format!("{}: {e}", path.display())))?,
            _ => serde_json::from_str::<AgentDefinition>(&text)
                .map_err(|e| TaskError::Invalid(format!("{}: {e}", path.display())))?,
        };
        if parsed.id.trim().is_empty()
            || parsed
                .id
                .contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        {
            return Err(TaskError::Invalid(format!(
                "invalid agent id in {}",
                path.display()
            )));
        }
        let mut parsed = parsed;
        parsed.source = path;
        definitions.insert(parsed.id.clone(), parsed);
    }
    if !definitions.contains_key("default") {
        definitions.insert(
            "default".into(),
            AgentDefinition {
                id: "default".into(),
                description: "Bundled general-purpose Jeden agent".into(),
                prompt: String::new(),
                tools: Vec::new(),
                model: None,
                output: serde_json::Value::Null,
                spawn: Default::default(),
                skills: Vec::new(),
                source: PathBuf::from("<bundled>"),
            },
        );
    }
    Ok(definitions.into_values().collect())
}
