//! What a registry entry actually runs.
//!
//! The catalog stores a label in `command` and, in its `catalog.agentHooks`
//! section, the file that label means in `source`. Most entries name an
//! absolute path, but the older ones read `shared-hooks/block_identity_literals.py`
//! — a path relative to the operator's home, not to the workspace a turn runs
//! in. Run from a checkout, that label names nothing: `sh` answers `No such
//! file or directory`, the exit code is non-zero, and a blocking event turns
//! that into a refusal of the tool. On 2026-09-15 that refused every file
//! write of every Jeden run on this machine, and the message the agent saw
//! named the file it was writing rather than the hook that could not start.
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
    let Some(program) = shell_words::split(written)
        .ok()
        .and_then(|tokens| tokens.into_iter().next())
    else {
        return Some(EntryCommand::Runnable(written.to_string()));
    };
    let path = Path::new(&program);
    // A bare name is a PATH lookup (`python3`, `node`, `cat`), which this
    // process has no business rewriting.
    if !program.contains('/') {
        return Some(EntryCommand::Runnable(written.to_string()));
    }
    if path.is_absolute() {
        return Some(if path.is_file() {
            EntryCommand::Runnable(written.to_string())
        } else {
            EntryCommand::Missing {
                written: written.to_string(),
                tried: vec![path.to_path_buf()],
            }
        });
    }
    let directory = registry.parent().unwrap_or(Path::new("."));
    let mut tried = Vec::with_capacity(3);
    if let Some(source) = catalog_source(catalog, entry) {
        tried.push(source);
    }
    tried.push(directory.join(path));
    if let Some(name) = path.file_name() {
        tried.push(directory.join(name));
    }
    match tried.iter().find(|candidate| candidate.is_file()) {
        Some(found) => Some(EntryCommand::Runnable(replace_program(written, found))),
        None => Some(EntryCommand::Missing {
            written: written.to_string(),
            tried,
        }),
    }
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
        Ok(tokens) if tokens.len() > 1 => {
            let rest = shell_words::join(tokens[1..].iter().map(String::as_str));
            format!("{quoted} {rest}")
        }
        _ => quoted,
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
