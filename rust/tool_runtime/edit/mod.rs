use serde_json::{json, Value};
use std::fs;

use super::shared::{bool_input, jail_path, sha256_hex, simple_diff, string_input, verify_expected_sha};
use super::ToolRuntime;

mod lines;
mod visual_apply;
mod visual_parse;

pub(crate) use visual_apply::visual_edit;

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

pub(crate) fn write_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("write_file requires --allow-write".into()); }
    let path = string_input(input, "path").ok_or("write_file requires path")?;
    let content = string_input(input, "content").ok_or("write_file requires content")?;
    let file = jail_path(runtime.cwd, &path)?;
    if file.exists() {
        let expected = string_input(input, "expectedSha256").ok_or("write_file overwrite requires expectedSha256")?;
        let old = fs::read(&file).map_err(|e| e.to_string())?;
        let actual = sha256_hex(&old);
        if actual != expected { return Err(format!("expectedSha256 mismatch for {path}: expected {expected}, actual {actual}")); }
    }
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::write(&file, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "path": path, "bytes": content.len(), "sha256": sha256_hex(content.as_bytes())}))
}

pub(crate) fn apply_patch_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("apply_patch requires --allow-write".into()); }
    let path = string_input(input, "path").ok_or("apply_patch requires path")?;
    let expected = string_input(input, "expectedSha256").ok_or("apply_patch requires expectedSha256")?;
    let file = jail_path(runtime.cwd, &path)?;
    let current_bytes = verify_expected_sha(&path, &file, &expected)?;
    let current = String::from_utf8(current_bytes).map_err(|e| e.to_string())?;
    let replacements = input.get("replacements").and_then(Value::as_array).ok_or("replacements are required")?;
    if replacements.is_empty() { return Err("replacements are required".into()); }
    let mut next = current.clone();
    for replacement in replacements {
        let old = replacement.get("old").and_then(Value::as_str).ok_or("old text is required")?;
        if old.is_empty() { return Err("old text is required".into()); }
        let new = replacement.get("new").and_then(Value::as_str).ok_or("new text is required")?;
        let first = next.find(old).ok_or("old text not found")?;
        if next[first + old.len()..].contains(old) { return Err("old text occurs more than once".into()); }
        next = format!("{}{}{}", &next[..first], new, &next[first + old.len()..]);
    }
    fs::write(&file, next.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "path": path, "sha256": sha256_hex(next.as_bytes()), "diff": simple_diff(&path, &current, &next), "replacements": replacements.len(), "bytes": next.len()}))
}

pub(crate) fn edit_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("edit_file requires --allow-write".into()); }
    let path = string_input(input, "path").ok_or("edit_file requires path")?;
    let expected = string_input(input, "expectedSha256").ok_or("edit_file requires expectedSha256")?;
    let file = jail_path(runtime.cwd, &path)?;
    let current_bytes = verify_expected_sha(&path, &file, &expected)?;
    let current = String::from_utf8(current_bytes).map_err(|e| e.to_string())?;
    let ops = input.get("ops").ok_or("ops are required")?;
    let next = lines::apply_line_edit_ops(&current, ops)?;
    fs::write(&file, next.as_bytes()).map_err(|e| e.to_string())?;
    let op_count = ops.as_array().map(Vec::len).unwrap_or(0);
    Ok(json!({"ok": true, "path": path, "sha256": sha256_hex(next.as_bytes()), "diff": simple_diff(&path, &current, &next), "ops": op_count, "bytes": next.len()}))
}

pub(crate) fn delete_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("delete_file requires --allow-write".into()); }
    let path = string_input(input, "path").ok_or("delete_file requires path")?;
    let expected = string_input(input, "expectedSha256").ok_or("delete_file requires expectedSha256")?;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = verify_expected_sha(&path, &file, &expected)?;
    fs::remove_file(&file).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "path": path, "deleted": true, "previousSha256": sha256_hex(&bytes), "previousBytes": bytes.len()}))
}

pub(crate) fn move_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("move_file requires --allow-write".into()); }
    let from = string_input(input, "from").ok_or("move_file requires from")?;
    let to = string_input(input, "to").ok_or("move_file requires to")?;
    let expected = string_input(input, "expectedSha256").ok_or("move_file requires expectedSha256")?;
    let overwrite = bool_input(input, "overwrite", false);
    let from_file = jail_path(runtime.cwd, &from)?;
    let to_file = jail_path(runtime.cwd, &to)?;
    let bytes = verify_expected_sha(&from, &from_file, &expected)?;
    if to_file.exists() && !overwrite { return Err(format!("destination exists: {to}")); }
    if let Some(parent) = to_file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::rename(&from_file, &to_file).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "from": from, "to": to, "sha256": sha256_hex(&bytes), "bytes": bytes.len()}))
}
