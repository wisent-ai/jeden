use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use super::ToolRuntime;

mod lines;

pub(crate) use lines::{
    selected_lines, simple_diff, snapshot_name, snapshot_tag, split_edit_lines, LineWindow,
};

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Resolve one tool path against the workspace root.
///
/// An absolute path inside the workspace is the same file as its relative
/// form, so it resolves rather than being refused. The bare refusal used to
/// leave the caller guessing: a turn asked to write
/// `<workspace>/alpha.txt`, read `absolute path rejected`, guessed the
/// relative form by keeping the last two segments, and created a phantom
/// `workspace/` directory inside the workspace that then satisfied nothing
/// the request asked for. An absolute path outside the workspace is still
/// refused, and the refusal names the root every path is taken from.
pub(crate) fn jail_path(cwd: &Path, input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    let raw = if trimmed.is_empty() { "." } else { trimmed };
    let absolute = Path::new(raw);
    let relative = if absolute.is_absolute() {
        match absolute.strip_prefix(cwd) {
            Ok(inside) if inside.as_os_str().is_empty() => ".".to_string(),
            Ok(inside) => inside.to_string_lossy().into_owned(),
            Err(_outside) => {
                return Err(format!(
                    "{raw} is outside this workspace; paths are taken from the workspace root {}",
                    cwd.display()
                ))
            }
        }
    } else {
        raw.to_string()
    };
    super::runtime_ops::fs::validate_relative(cwd, &relative).map_err(|error| error.to_string())
}

/// The directory a workspace keeps Jeden's own state in: the session pointer,
/// mode state, the usage ledger and the retained completion state.
pub(crate) const WORKSPACE_STATE_DIR: &str = ".jeden";

/// Resolve a path a tool is about to change, refusing the harness's own state
/// directory.
///
/// A turn that writes the files its assignment asked for into `.jeden/` and
/// reports the work done leaves the workspace root holding neither file, while
/// the directory that holds the record the work is judged by holds them
/// instead. Session pointers, mode state, the usage ledger and completion
/// state live there, so a tool write there rewrites that record. Reads stay
/// allowed; only changes are refused.
pub(crate) fn jail_write_path(cwd: &Path, input: &str) -> Result<PathBuf, String> {
    let path = jail_path(cwd, input)?;
    let inside_state = path.strip_prefix(cwd).is_ok_and(|relative| {
        relative
            .components()
            .next()
            .is_some_and(|first| first.as_os_str() == WORKSPACE_STATE_DIR)
    });
    if inside_state {
        return Err(format!(
            "{input}: {WORKSPACE_STATE_DIR}/ holds this workspace's Jeden state (sessions, mode, usage, retained completion) and no tool may change it; write the work where the request asks for it"
        ));
    }
    Ok(path)
}

pub(crate) fn string_input(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The archive formats a `path` may read or write into as `<archive>:<entry>`.
pub(crate) const ARCHIVE_SUFFIXES: &[&str] = &[".tar.gz", ".tgz", ".tar", ".zip"];

/// The database formats a `path` may address as `<database>:<table>[:<key>]`.
pub(crate) const DATABASE_SUFFIXES: &[&str] = &[".sqlite", ".db"];

/// `path` split at the first `<suffix>:` of any of `suffixes`, matched without
/// regard to case: the container file and what follows the colon. `None`
/// when the path names no such container.
pub(crate) fn container_path<'a>(path: &'a str, suffixes: &[&str]) -> Option<(&'a str, &'a str)> {
    let lower = path.to_ascii_lowercase();
    suffixes.iter().find_map(|suffix| {
        let index = lower.find(&format!("{suffix}:"))?;
        let (container, rest) = path.split_at(index + suffix.len());
        rest.strip_prefix(':').map(|inner| (container, inner))
    })
}

/// A yes/no the tool's schema lets the caller leave out: `None` when it is
/// absent, refused by name when it is present but not a boolean.
pub(crate) fn optional_bool_input(
    input: &Value,
    key: &str,
    tool: &str,
) -> Result<Option<bool>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(other) => Err(format!("{tool} {key} must be true or false, not {other}")),
    }
}

/// A switch that is on only when the caller sets it to true.
pub(crate) fn flag_input(input: &Value, key: &str, tool: &str) -> Result<bool, String> {
    Ok(optional_bool_input(input, key, tool)? == Some(true))
}

/// A count the tool's schema lets the caller leave out — how many leading
/// matches to pass over, how many levels to descend. The default is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Amount(pub(crate) usize);

/// The amount stated under `key`: none when it is absent, refused by name
/// when it is present but not a whole number this machine can count to.
pub(crate) fn amount_input(input: &Value, key: &str, tool: &str) -> Result<Amount, String> {
    let value = match input.get(key) {
        None | Some(Value::Null) => return Ok(Amount::default()),
        Some(value) => value,
    };
    let whole = value
        .as_u64()
        .ok_or_else(|| format!("{tool} {key} must be a whole number, not {value}"))?;
    usize::try_from(whole).map(Amount).map_err(|error| {
        format!("{tool} {key} {whole} is more than this machine can count: {error}")
    })
}

/// An amount a tool works to — how many entries it answers, how many bytes
/// it reads — as the caller states it, refused by name when it is missing or
/// not a positive whole number. No tool picks an amount for the model, and
/// none caps the one it was given.
pub(crate) fn count_input(input: &Value, key: &str, tool: &str) -> Result<u64, String> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .and_then(std::num::NonZeroU64::new)
        .map(std::num::NonZeroU64::get)
        .ok_or_else(|| format!("{tool} requires {key}, a positive whole number the caller states"))
}

/// How many threads this machine runs, read from the operating system and
/// refused by name when it cannot be read; a search fans out across all of
/// them.
pub(crate) fn machine_parallelism(tool: &str) -> Result<usize, String> {
    std::thread::available_parallelism()
        .map(usize::from)
        .map_err(|error| {
            format!("{tool} could not read how many threads this machine runs: {error}")
        })
}

/// The arguments object a call passes on: no arguments when `key` is absent,
/// refused by name when it is present but not an object.
pub(crate) fn arguments_input(input: &Value, key: &str, tool: &str) -> Result<Value, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(Value::Object(Map::new())),
        Some(Value::Object(arguments)) => Ok(Value::Object(arguments.clone())),
        Some(other) => Err(format!("{tool} {key} must be an object, not {other}")),
    }
}

/// The media type `bytes` carry, read from the bytes themselves: an image
/// format's own signature, a PDF's file header, UTF-8 text (a character cut
/// by a read limit at the end still counts), or `application/octet-stream`.
/// A file's name is not asked: an extension table missed every format it did
/// not list (`.py`, `.go`, `.sh`, …) and trusted a name over the content.
pub(crate) fn mime_type_of(bytes: &[u8]) -> &'static str {
    if let Some(image) = super::read::image_signature(bytes) {
        return image;
    }
    if bytes.starts_with(super::read::PDF_SIGNATURE) {
        return super::read::PDF_TYPE;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => super::read::TEXT_TYPE,
        Err(error) if error.error_len().is_none() => super::read::TEXT_TYPE,
        Err(_invalid) => super::read::BINARY_TYPE,
    }
}

pub(crate) fn verify_expected_sha(
    path_label: &str,
    file: &Path,
    expected: &str,
) -> Result<Vec<u8>, String> {
    let bytes = fs::read(file).map_err(|e| e.to_string())?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(format!(
            "expectedSha256 mismatch for {path_label}: expected {expected}, actual {actual}"
        ));
    }
    Ok(bytes)
}

pub(crate) fn run_read_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let elevated = ToolRuntime {
        cwd: runtime.cwd,
        artifact_dir: runtime.artifact_dir,
        operation: runtime.operation.clone(),
        allow_write: runtime.allow_write,
        allow_command: true,
        interactive: runtime.interactive,
        ask_user: runtime.ask_user,
    };
    super::exec::run_process(&elevated, input)
}
