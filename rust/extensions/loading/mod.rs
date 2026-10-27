//! Finding what there is to load: which files are extension modules, which
//! declarative capabilities a root declares, and the fingerprint that says
//! whether anything under those roots has changed since the last generation.

use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::hash::{DefaultHasher, Hash};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::DeclarativeCapability;

pub(crate) mod host;
mod host_stream;
pub(crate) mod materialize;
pub(crate) mod roots;

/// The key a working directory's registry is kept under: the directory
/// resolved. One that cannot be resolved is keyed as written and the cause
/// is said; the source scan that follows refuses what it cannot read.
pub(super) fn canonical_key(cwd: &Path) -> PathBuf {
    match fs::canonicalize(cwd) {
        Ok(resolved) => resolved,
        Err(unresolved) => {
            eprintln!(
                "jeden: the extension registry of {} is keyed as written, since it cannot be resolved: {unresolved}",
                cwd.display()
            );
            cwd.to_path_buf()
        }
    }
}

/// The module formats the extension host loads, named by file extension.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModuleExtension {
    Js,
    Mjs,
    Ts,
}

pub(crate) fn module_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                serde_json::from_value::<ModuleExtension>(Value::from(extension)).is_ok()
            })
}

/// The extension modules a root's `package.json` declares (`jeden.extensions`,
/// else `pi.extensions`). No manifest declares none; one that cannot be read
/// or parsed, or that declares an entry which is not a module file, is
/// refused naming it: skipped, the extensions it declared silently did not
/// load.
pub(super) fn package_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    let path = root.join("package.json");
    let manifest = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| format!("{} is not JSON: {error}", path.display()))?,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    let entries = manifest
        .pointer("/jeden/extensions")
        .and_then(Value::as_array)
        .or_else(|| manifest.pointer("/pi/extensions").and_then(Value::as_array));
    entries
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|entry| {
            let module = root.join(entry);
            if module_file(&module) {
                Ok(module)
            } else {
                Err(format!(
                    "{} declares the extension {entry}, which is not a module file",
                    path.display()
                ))
            }
        })
        .collect()
}

/// The extension modules under `root`: a module file itself, a package's
/// declared entries, an `index` module, or every module file in it (and, with
/// `recursive_children`, in its child directories). A root that does not
/// exist holds none; one that cannot be examined or listed is refused naming
/// it, never read as a root without extensions.
pub(crate) fn scan_modules(root: &Path, recursive_children: bool) -> Result<Vec<PathBuf>, String> {
    if !crate::tool_runtime::runtime_ops::platform::presence::present(root)? {
        return Ok(Vec::new());
    }
    if module_file(root) {
        return Ok(vec![root.to_path_buf()]);
    }
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let manifest = package_entries(root)?;
    if !manifest.is_empty() {
        return Ok(manifest);
    }
    for name in ["index.ts", "index.js", "index.mjs"] {
        let path = root.join(name);
        if module_file(&path) {
            return Ok(vec![path]);
        }
    }
    let mut files = Vec::new();
    let unlistable = |error: std::io::Error| {
        format!("cannot list the extensions in {}: {error}", root.display())
    };
    let entries = fs::read_dir(root).map_err(unlistable)?;
    for entry in entries {
        let path = entry.map_err(unlistable)?.path();
        if module_file(&path) {
            files.push(path);
        } else if recursive_children && path.is_dir() {
            files.extend(scan_modules(&path, false)?);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// What a root may declare besides modules. Each kind is a directory of
/// files under the root, except hooks, which is one manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeclarativeKind {
    Commands,
    Hooks,
    Skills,
    Agents,
    Rules,
}

impl DeclarativeKind {
    const EVERY: &'static [DeclarativeKind] = &[
        DeclarativeKind::Commands,
        DeclarativeKind::Hooks,
        DeclarativeKind::Skills,
        DeclarativeKind::Agents,
        DeclarativeKind::Rules,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            DeclarativeKind::Commands => "commands",
            DeclarativeKind::Hooks => "hooks",
            DeclarativeKind::Skills => "skills",
            DeclarativeKind::Agents => "agents",
            DeclarativeKind::Rules => "rules",
        }
    }

    /// The kind `name` spells, read back through [`DeclarativeKind::name`].
    pub(crate) fn named(name: &str) -> Option<DeclarativeKind> {
        Self::EVERY.iter().copied().find(|kind| kind.name() == name)
    }

    /// The capability each definition of this kind is listed as, for the
    /// kinds the declarative runtime loads; none for those the registry
    /// carries itself.
    pub(crate) fn capability(self) -> Option<crate::capability::CapabilityKind> {
        use crate::capability::CapabilityKind;
        match self {
            DeclarativeKind::Skills => Some(CapabilityKind::Skill),
            DeclarativeKind::Agents => Some(CapabilityKind::Agent),
            DeclarativeKind::Rules => Some(CapabilityKind::Rule),
            DeclarativeKind::Commands | DeclarativeKind::Hooks => None,
        }
    }

    /// Whether the registry itself carries this kind's capability — the
    /// commands it expands and the hooks it runs — rather than handing it to
    /// the declarative runtime.
    fn registry_carries(self) -> bool {
        matches!(self, DeclarativeKind::Commands | DeclarativeKind::Hooks)
    }

    fn path(self, root: &Path) -> PathBuf {
        match self {
            DeclarativeKind::Hooks => root.join("hooks.json"),
            _ => root.join(self.name()),
        }
    }

    fn health(self, path: &Path) -> Result<(), String> {
        match self {
            DeclarativeKind::Hooks => {
                let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
                let value =
                    serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())?;
                if value.is_object() {
                    Ok(())
                } else {
                    Err("hooks manifest must be an object".into())
                }
            }
            _ => fs::read_dir(path)
                .map(drop)
                .map_err(|error| error.to_string()),
        }
    }
}

pub(super) fn declarative_paths(root: &Path, precedence: usize) -> Vec<DeclarativeCapability> {
    let mut values = Vec::new();
    for kind in DeclarativeKind::EVERY.iter().copied() {
        let path = kind.path(root);
        // A path that cannot be examined is listed unhealthy with its cause:
        // read as absent, the capability vanished from the listing.
        let health = match crate::tool_runtime::runtime_ops::platform::presence::present(&path) {
            Ok(false) => continue,
            Ok(true) => kind.health(&path),
            Err(cause) => Err(cause),
        };
        values.push(DeclarativeCapability {
            kind: kind.name(),
            carried: kind.registry_carries(),
            path,
            healthy: health.is_ok(),
            error: health.err(),
            precedence,
        });
    }
    values
}
/// Hash what is under `path`: names, sizes and modification times. What
/// cannot be read is hashed as its error's kind, so a tree that turns
/// readable or unreadable changes the fingerprint as well.
pub(super) fn hash_path_tree(path: &Path, hasher: &mut DefaultHasher) {
    path.hash(hasher);
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return error.kind().hash(hasher),
    };
    metadata.len().hash(hasher);
    match metadata.modified() {
        Ok(modified) => modified.hash(hasher),
        Err(error) => error.kind().hash(hasher),
    }
    if metadata.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => return error.kind().hash(hasher),
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => children.push(entry.path()),
                Err(error) => error.kind().hash(hasher),
            }
        }
        children.sort();
        for child in children {
            hash_path_tree(&child, hasher);
        }
    }
}

/// `key` as the configuration layers that apply here declare it, a later
/// layer (the project) overriding an earlier one (the user's files). A layer
/// that cannot be read or does not parse is refused naming the file: read as
/// empty, an invalid project configuration dropped `disabledExtensions` and
/// loaded every extension it switched off.
pub(super) fn config_value(cwd: &Path, key: &str) -> Result<Value, String> {
    let mut declared = Value::Null;
    for path in crate::cli::config::config_layer_paths(cwd) {
        let Some(layer) = crate::cli::config::read_config_layer(&path)? else {
            continue;
        };
        if let Some(value) = layer.get(key) {
            declared = value.clone();
        }
    }
    Ok(declared)
}
