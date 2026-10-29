use serde_json::{json, Value};
use std::fs;

use super::shared::{
    container_path, flag_input, jail_write_path, sha256_hex, simple_diff, string_input,
    verify_expected_sha, ARCHIVE_SUFFIXES, DATABASE_SUFFIXES,
};
use super::ToolRuntime;

mod lines;
mod notebook;
mod storage;
mod subjects;
mod visual_apply;
mod visual_parse;

pub(crate) use storage::{write_archive, write_sqlite};
pub(crate) use subjects::policy_subjects;
pub(crate) use visual_apply::visual_edit;

/// The refusal for one filesystem step of an edit, naming what was being done
/// to which path: the tool's caller sees which write failed, not a bare
/// operating-system message.
fn io_failed<'a>(
    action: &'static str,
    path: &'a std::path::Path,
) -> impl FnOnce(std::io::Error) -> String + 'a {
    move |error| format!("cannot {action} {}: {error}", path.display())
}

/// A file the edit reads as text that is not UTF-8, named.
fn not_text(path: &std::path::Path) -> impl FnOnce(std::string::FromUtf8Error) -> String + '_ {
    move |error| {
        format!(
            "{} is not UTF-8 text, so it cannot be edited as text: {error}",
            path.display()
        )
    }
}

#[derive(Clone)]
struct VisualPatchOp {
    op: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
    line: Option<usize>,
    content: Vec<String>,
}

struct VisualPatchSection {
    path: String,
    tag: String,
    ops: Vec<VisualPatchOp>,
    remove: bool,
    move_to: Option<String>,
}

pub(crate) fn write_any(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("write requires path")?;
    if let Some((archive, entry)) = container_path(&path, ARCHIVE_SUFFIXES) {
        let mut routed = input.clone();
        let object = routed
            .as_object_mut()
            .ok_or("write input must be an object")?;
        object.insert("path".into(), json!(archive));
        object.insert("entry".into(), json!(entry));
        // Writing an entry's content is upserting it.
        object.insert("action".into(), json!("upsert"));
        return write_archive(runtime, &routed);
    }
    let lower = path.to_ascii_lowercase();
    if DATABASE_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
        && input.get("table").is_some()
    {
        return write_sqlite(runtime, input);
    }
    write_file(runtime, input)
}

pub(crate) fn write_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("write_file requires --allow-write".into());
    }
    let path = string_input(input, "path").ok_or("write_file requires path")?;
    let content = string_input(input, "content").ok_or("write_file requires content")?;
    let file = jail_write_path(runtime.cwd, &path)?;
    // The overwrite guard reads the file itself: `exists()` answered false
    // for a file it could not stat, and the write then went ahead unguarded.
    match fs::read(&file) {
        Ok(old) => {
            let expected = string_input(input, "expectedSha256")
                .ok_or("write_file overwrite requires expectedSha256")?;
            let actual = sha256_hex(&old);
            if actual != expected {
                return Err(format!(
                    "expectedSha256 mismatch for {path}: expected {expected}, actual {actual}"
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{path} cannot be read: {error}")),
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(io_failed("create directory", parent))?;
    }
    let bytes = notebook::notebook_bytes(&file, &content)?;
    fs::write(&file, &bytes).map_err(io_failed("write", &file))?;
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "sha256": sha256_hex(&bytes)}))
}

pub(crate) fn apply_patch_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("apply_patch requires --allow-write".into());
    }
    let path = string_input(input, "path").ok_or("apply_patch requires path")?;
    let expected =
        string_input(input, "expectedSha256").ok_or("apply_patch requires expectedSha256")?;
    let file = jail_write_path(runtime.cwd, &path)?;
    let current_bytes = verify_expected_sha(&path, &file, &expected)?;
    let current = String::from_utf8(current_bytes).map_err(not_text(&file))?;
    let replacements = input
        .get("replacements")
        .ok_or("replacements are required")?;
    let next = replaced(&current, replacements)?;
    fs::write(&file, next.as_bytes()).map_err(io_failed("write", &file))?;
    Ok(
        json!({"ok": true, "path": path, "sha256": sha256_hex(next.as_bytes()), "diff": simple_diff(&path, &current, &next), "replacements": replacements.as_array().map(Vec::len), "bytes": next.len()}),
    )
}

/// `current` with each exact, single-occurrence replacement applied in order;
/// the tool and its policy hooks read the same result.
fn replaced(current: &str, replacements: &Value) -> Result<String, String> {
    let replacements = replacements.as_array().ok_or("replacements are required")?;
    if replacements.is_empty() {
        return Err("replacements are required".into());
    }
    let mut next = current.to_owned();
    for replacement in replacements {
        let old = replacement
            .get("old")
            .and_then(Value::as_str)
            .ok_or("old text is required")?;
        if old.is_empty() {
            return Err("old text is required".into());
        }
        let new = replacement
            .get("new")
            .and_then(Value::as_str)
            .ok_or("new text is required")?;
        let first = next.find(old).ok_or("old text not found")?;
        if next[first + old.len()..].contains(old) {
            return Err("old text occurs more than once".into());
        }
        next = format!("{}{}{}", &next[..first], new, &next[first + old.len()..]);
    }
    Ok(next)
}

pub(crate) fn edit_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("edit_file requires --allow-write".into());
    }
    let path = string_input(input, "path").ok_or("edit_file requires path")?;
    let expected =
        string_input(input, "expectedSha256").ok_or("edit_file requires expectedSha256")?;
    let file = jail_write_path(runtime.cwd, &path)?;
    let current_bytes = verify_expected_sha(&path, &file, &expected)?;
    let current = String::from_utf8(current_bytes).map_err(not_text(&file))?;
    let ops = input.get("ops").ok_or("ops are required")?;
    let op_count = ops
        .as_array()
        .map(Vec::len)
        .ok_or_else(|| format!("edit_file ops must be a list, not {ops}"))?;
    let next = lines::apply_line_edit_ops(&current, ops)?;
    fs::write(&file, next.as_bytes()).map_err(io_failed("write", &file))?;
    Ok(
        json!({"ok": true, "path": path, "sha256": sha256_hex(next.as_bytes()), "diff": simple_diff(&path, &current, &next), "ops": op_count, "bytes": next.len()}),
    )
}

pub(crate) fn delete_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("delete_file requires --allow-write".into());
    }
    let path = string_input(input, "path").ok_or("delete_file requires path")?;
    let expected =
        string_input(input, "expectedSha256").ok_or("delete_file requires expectedSha256")?;
    let file = jail_write_path(runtime.cwd, &path)?;
    let bytes = verify_expected_sha(&path, &file, &expected)?;
    fs::remove_file(&file).map_err(io_failed("delete", &file))?;
    Ok(
        json!({"ok": true, "path": path, "deleted": true, "previousSha256": sha256_hex(&bytes), "previousBytes": bytes.len()}),
    )
}

pub(crate) fn move_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("move_file requires --allow-write".into());
    }
    let from = string_input(input, "from").ok_or("move_file requires from")?;
    let to = string_input(input, "to").ok_or("move_file requires to")?;
    let expected =
        string_input(input, "expectedSha256").ok_or("move_file requires expectedSha256")?;
    let overwrite = flag_input(input, "overwrite", "move_file")?;
    let from_file = jail_write_path(runtime.cwd, &from)?;
    let to_file = jail_write_path(runtime.cwd, &to)?;
    let bytes = verify_expected_sha(&from, &from_file, &expected)?;
    // A destination that cannot be examined is refused: read as absent, the
    // move replaced it without overwrite being given.
    if !overwrite && crate::tool_runtime::runtime_ops::platform::presence::present(&to_file)? {
        return Err(format!("destination exists: {to}"));
    }
    if let Some(parent) = to_file.parent() {
        fs::create_dir_all(parent).map_err(io_failed("create directory", parent))?;
    }
    fs::rename(&from_file, &to_file).map_err(|error| {
        format!(
            "cannot move {} to {}: {error}",
            from_file.display(),
            to_file.display()
        )
    })?;
    Ok(
        json!({"ok": true, "from": from, "to": to, "sha256": sha256_hex(&bytes), "bytes": bytes.len()}),
    )
}
