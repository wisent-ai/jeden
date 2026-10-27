//! What a registry entry actually runs.
//!
//! The catalog stores a label in `command` and, in its `catalog.agentHooks`
//! section, the file that label means in `source`. Most entries name an
//! absolute path, but the older ones read `shared-hooks/block_identity_literals.py`
//! — a path relative to the operator's home, not to the workspace a turn runs
//! in. Run from a checkout, that label names nothing: `sh` answers `No such
//! file or directory`, the exit code is non-zero, and a blocking event turns
//! that into a refusal of the tool — of every file write of every Jeden run
//! on the machine, with a message that names the file being written rather
//! than the hook that could not start.
//!
//! So a relative program is resolved against the registry itself, which is the
//! only directory the registration can be talking about, and a registration
//! that resolves nowhere is reported as what it is.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The executable of a registry entry: a non-empty `command` with no `type` or
/// `type: "command"`. Other entry kinds cannot run natively here.
pub(super) enum EntryCommand {
    /// A command line this process can run as written.
    Runnable(String),
    /// A registration whose executable is absent, and the paths tried for it.
    Missing {
        written: String,
        tried: Vec<PathBuf>,
    },
    /// A registration whose executable could not be examined, with why: never
    /// read as absent, which would run a later candidate in its place.
    Unexamined { written: String, reason: String },
}

pub(super) fn entry_command(
    entry: &Value,
    registry: &Path,
    catalog: &Value,
) -> Option<EntryCommand> {
    match entry.get("type").and_then(Value::as_str) {
        Some("command") | None => {}
        Some(_) => return None,
    }
    let written = entry.get("command").and_then(Value::as_str)?.trim();
    if written.is_empty() {
        return None;
    }
    // A command line the shell-word reader cannot split, or one with no word,
    // is handed to `sh` as written; `sh` then says what is wrong with it.
    let program = match shell_words::split(written).map(|tokens| tokens.into_iter().next()) {
        Ok(Some(program)) => program,
        Ok(None) => return Some(EntryCommand::Runnable(written.to_string())),
        Err(_shell_reports_it) => return Some(EntryCommand::Runnable(written.to_string())),
    };
    let path = Path::new(&program);
    // A bare name is a PATH lookup (`python3`, `node`, `cat`), which this
    // process has no business rewriting.
    if !program.contains('/') {
        return Some(EntryCommand::Runnable(written.to_string()));
    }
    if path.is_absolute() {
        return Some(match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => EntryCommand::Runnable(written.to_string()),
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => EntryCommand::Unexamined {
                written: written.to_string(),
                reason: format!("{} cannot be examined: {error}", path.display()),
            },
            _absent_or_not_file => EntryCommand::Missing {
                written: written.to_string(),
                tried: vec![path.to_path_buf()],
            },
        });
    }
    // A relative program is looked for beside the registry that names it; a
    // registry named by a bare file name lives in the working directory.
    let directory = match registry.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _working_directory => Path::new("."),
    };
    let mut tried = Vec::new();
    if let Some(source) = catalog_source(catalog, entry) {
        tried.push(source);
    }
    tried.push(directory.join(path));
    if let Some(name) = path.file_name() {
        tried.push(directory.join(name));
    }
    for candidate in &tried {
        match std::fs::metadata(candidate) {
            Ok(metadata) if metadata.is_file() => {
                return Some(EntryCommand::Runnable(replace_program(written, candidate)))
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Some(EntryCommand::Unexamined {
                    written: written.to_string(),
                    reason: format!("{} cannot be examined: {error}", candidate.display()),
                })
            }
            _absent_or_not_file => {}
        }
    }
    Some(EntryCommand::Missing {
        written: written.to_string(),
        tried,
    })
}

/// The absolute `source` the catalog records for this entry's id, when it has
/// one. The catalog is the registry's own statement of which file each hook is.
fn catalog_source(catalog: &Value, entry: &Value) -> Option<PathBuf> {
    let id = entry.get("id").and_then(Value::as_str)?;
    let source = catalog
        .get("agentHooks")
        .and_then(Value::as_array)?
        .iter()
        .find(|known| known.get("id").and_then(Value::as_str) == Some(id))?
        .get("source")
        .and_then(Value::as_str)?;
    let path = PathBuf::from(source.trim());
    path.is_absolute().then_some(path)
}

/// The same command line with its first token replaced by `program`, quoted so
/// a path holding a space still runs as one word.
fn replace_program(written: &str, program: &Path) -> String {
    let quoted = shell_words::quote(&program.to_string_lossy()).into_owned();
    match shell_words::split(written) {
        Ok(tokens) => match tokens.split_first() {
            Some((_program, rest)) if !rest.is_empty() => {
                format!("{quoted} {}", shell_words::join(rest.iter().map(String::as_str)))
            }
            _no_arguments => quoted,
        },
        Err(_unsplittable) => quoted,
    }
}

/// What a turn is told when a registered hook names a file this machine does
/// not have. Blocking is left to the caller: this is the sentence, not the
/// verdict.
pub(super) fn unrunnable_reason(
    id: &str,
    written: &str,
    tried: &[PathBuf],
    registry: &Path,
) -> String {
    let named = if id.is_empty() { "a hook" } else { id };
    let paths = tried
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "TAMA_HOOK_INFRASTRUCTURE: the hook `{named}` is registered in {} as `{written}`, \
         and no such file exists here (tried: {paths}). The tool was refused because a \
         registered hook that cannot start has not judged it.",
        registry.display()
    )
}
