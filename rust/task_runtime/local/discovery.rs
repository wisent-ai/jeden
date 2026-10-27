use crate::task_runtime::types::{AgentDefinition, TaskError};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The formats an agent definition file is written in, named by its
/// extension: the variant's own name, `yml` being YAML too. Delegation and
/// the extension loader read definitions through this one declaration.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DefinitionFormat {
    Json,
    #[serde(alias = "yml")]
    Yaml,
}

impl DefinitionFormat {
    /// The format `path`'s extension names, or none for a file that is not
    /// an agent definition: its extension names no variant above, which is
    /// all the parse of the extension can refuse.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        match serde_json::from_value(serde_json::Value::String(extension.to_owned())) {
            Ok(format) => Some(format),
            Err(_another_kind_of_file) => None,
        }
    }

    /// `text` read in this format as `T`, refused with the parser's cause.
    pub(crate) fn parse<T: serde::de::DeserializeOwned>(self, text: &str) -> Result<T, String> {
        match self {
            Self::Json => serde_json::from_str(text).map_err(|error| error.to_string()),
            Self::Yaml => serde_yaml::from_str(text).map_err(|error| error.to_string()),
        }
    }
}

/// The characters an agent id may hold besides ASCII letters and digits.
const ID_PUNCTUATION: &[char] = &['-', '_'];

/// Whether `id` names an agent delegation can address.
pub(crate) fn valid_agent_id(id: &str) -> bool {
    !id.trim().is_empty()
        && !id.contains(|c: char| !c.is_ascii_alphanumeric() && !ID_PUNCTUATION.contains(&c))
}

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
/// none; one that cannot be examined, or a directory that cannot be listed,
/// is refused naming it.
fn collect(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), TaskError> {
    if !crate::tool_runtime::runtime_ops::platform::presence::present(root).map_err(TaskError::Io)? {
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
        let format = DefinitionFormat::of(&path).ok_or_else(|| {
            TaskError::Invalid(format!(
                "{} names no agent definition format by its extension",
                path.display()
            ))
        })?;
        let parsed = format
            .parse::<AgentDefinition>(&text)
            .map_err(|e| TaskError::Invalid(format!("{}: {e}", path.display())))?;
        if !valid_agent_id(&parsed.id) {
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
