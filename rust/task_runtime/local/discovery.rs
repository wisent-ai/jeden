use crate::task_runtime::types::{AgentDefinition, TaskError};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The formats an agent definition file is written in, by extension.
#[derive(Clone, Copy)]
enum DefinitionFormat {
    Json,
    Yaml,
}

impl DefinitionFormat {
    fn of(path: &Path) -> Option<Self> {
        match path.extension().and_then(|extension| extension.to_str())? {
            "json" => Some(Self::Json),
            "yaml" => Some(Self::Yaml),
            "yml" => Some(Self::Yaml),
            _ => None,
        }
    }
}

/// The characters an agent id may hold besides ASCII letters and digits.
const ID_PUNCTUATION: &[char] = &['-', '_'];

/// Where agent definitions are read: the bundled agents, the user's, the
/// project's, and every directory an active extension declares. Extensions
/// that cannot be read are refused with their cause: skipped, the agents
/// they declare silently vanished from delegation.
fn roots(cwd: &Path) -> Result<Vec<PathBuf>, TaskError> {
    let user = crate::dirs_home().join(".jeden");
    let mut roots = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("agents"),
        user.join("agents"),
        cwd.join(".jeden/agents"),
    ];
    roots.extend(crate::hooks::extension_agent_dirs(cwd).map_err(TaskError::Invalid)?);
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Every definition file under `root`. A root that does not exist holds
/// none; a directory that cannot be listed is refused naming it.
fn collect(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), TaskError> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if DefinitionFormat::of(root).is_some() {
            out.push(root.to_path_buf());
        }
        return Ok(());
    }
    let unlistable = |error: std::io::Error| {
        TaskError::Io(format!(
            "cannot list agent definitions in {}: {error}",
            root.display()
        ))
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).map_err(unlistable)? {
        paths.push(entry.map_err(unlistable)?.path());
    }
    paths.sort();
    for path in paths {
        collect(&path, out)?;
    }
    Ok(())
}

pub fn discover_agents(cwd: &Path) -> Result<Vec<AgentDefinition>, TaskError> {
    let mut files = Vec::new();
    for root in roots(cwd)? {
        collect(&root, &mut files)?;
    }
    let mut definitions = BTreeMap::new();
    for path in files {
        let text = fs::read_to_string(&path).map_err(|error| {
            TaskError::Io(format!("{} cannot be read: {error}", path.display()))
        })?;
        let parsed = match DefinitionFormat::of(&path) {
            Some(DefinitionFormat::Yaml) => serde_yaml::from_str::<AgentDefinition>(&text)
                .map_err(|e| TaskError::Invalid(format!("{}: {e}", path.display())))?,
            _ => serde_json::from_str::<AgentDefinition>(&text)
                .map_err(|e| TaskError::Invalid(format!("{}: {e}", path.display())))?,
        };
        if parsed.id.trim().is_empty()
            || parsed
                .id
                .contains(|c: char| !c.is_ascii_alphanumeric() && !ID_PUNCTUATION.contains(&c))
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
