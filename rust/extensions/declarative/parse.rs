//! Reading a definition file without letting it reach outside itself.
//!
//! Split out of `extensions/declarative.rs`, which had grown past the module
//! line cap.

use regex::Regex;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::task_runtime::discovery::DefinitionFormat;

pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// How a declarative file is written: Markdown with a frontmatter block, or
/// a structured definition in one of the formats delegation reads agents in.
#[derive(Clone, Copy)]
pub(super) enum Notation {
    Markdown,
    Structured(DefinitionFormat),
}

/// The extension a Markdown definition carries.
const MARKDOWN_EXTENSION: &str = "md";

impl Notation {
    pub(super) fn of(path: &Path) -> Option<Self> {
        if let Some(format) = DefinitionFormat::of(path) {
            return Some(Self::Structured(format));
        }
        (path.extension()? == MARKDOWN_EXTENSION).then_some(Self::Markdown)
    }

    pub(super) fn markdown(self) -> bool {
        matches!(self, Self::Markdown)
    }

    pub(super) fn structured(self) -> bool {
        matches!(self, Self::Structured(_))
    }
}

/// Every file under `root` written in a notation `accepts` takes.
pub(super) fn collect(root: &Path, accepts: fn(Notation) -> bool, out: &mut Vec<PathBuf>) {
    match crate::tool_runtime::runtime_ops::platform::presence::present(root) {
        Ok(true) => {}
        Ok(false) => return,
        Err(cause) => {
            eprintln!("jeden: declarative extensions were not loaded: {cause}");
            return;
        }
    }
    if root.is_file() {
        if Notation::of(root).is_some_and(accepts) {
            out.push(root.to_path_buf());
        }
        return;
    }
    // A directory that exists but cannot be listed is said with its cause:
    // skipped in silence, every definition under it vanished from the
    // session as if it had never been written.
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "jeden: declarative extensions under {} were not loaded: {error}",
                root.display()
            );
            return;
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => eprintln!(
                "jeden: a declarative extension under {} was not loaded: {error}",
                root.display()
            ),
        }
    }
    paths.sort();
    for path in paths {
        collect(&path, accepts, out);
    }
}

/// The line that opens YAML frontmatter, and the one that closes it.
const FRONTMATTER_OPEN: &str = "---\n";
const FRONTMATTER_CLOSE: &str = "\n---\n";

pub(super) fn parse_frontmatter(text: &str) -> Result<(Map<String, Value>, String), String> {
    let normalized = text.replace("\r\n", "\n");
    if !normalized.starts_with(FRONTMATTER_OPEN) {
        return Ok((Map::new(), normalized));
    }
    let tail = &normalized[FRONTMATTER_OPEN.len()..];
    let (front, body) = tail
        .split_once(FRONTMATTER_CLOSE)
        .ok_or("unterminated YAML frontmatter")?;
    let metadata: Value = serde_yaml::from_str(front).map_err(|error| error.to_string())?;
    let metadata = metadata
        .as_object()
        .cloned()
        .ok_or("frontmatter must be an object")?;
    Ok((metadata, body.to_string()))
}

pub(super) fn string_list(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) => Ok(vec![value.clone()]),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "list entries must be strings".to_string())
            })
            .collect(),
        Some(_) => Err("expected a string or string array".into()),
    }
}

pub(super) fn validate_matchers(matchers: &[String]) -> Result<(), String> {
    for matcher in matchers {
        Regex::new(matcher).map_err(|error| format!("invalid matcher {matcher:?}: {error}"))?;
    }
    Ok(())
}

pub(super) fn matches(matchers: &[String], prompt: &str) -> bool {
    matchers
        .iter()
        .any(|matcher| Regex::new(matcher).is_ok_and(|regex| regex.is_match(prompt)))
}

pub(super) fn safe_assets(skill_file: &Path, raw: Option<&Value>) -> Result<Vec<PathBuf>, String> {
    let relative = string_list(raw)?;
    let root = skill_file
        .parent()
        .ok_or_else(|| format!("skill file {} has no directory to hold its assets", skill_file.display()))?;
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let mut assets = Vec::new();
    for asset in relative {
        let candidate = root.join(&asset);
        let canonical = fs::canonicalize(&candidate)
            .map_err(|error| format!("skill asset {asset}: {error}"))?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
            return Err(format!("unsafe skill asset: {asset}"));
        }
        assets.push(canonical);
    }
    Ok(assets)
}

/// A skill's id: its file's name, or for a `SKILL.md` the name of the
/// directory that holds it. Names are read as text, a byte that is not
/// UTF-8 shown as the replacement character rather than replaced by a word.
pub(crate) fn skill_file_id(path: &Path) -> String {
    let file = match path.file_stem() {
        Some(stem) => stem.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    };
    if !file.eq_ignore_ascii_case("skill") {
        return file;
    }
    match path.parent().and_then(Path::file_name) {
        Some(directory) => directory.to_string_lossy().into_owned(),
        None => file,
    }
}
