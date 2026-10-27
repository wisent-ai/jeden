use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use super::lines::apply_line_edit_ops;
use super::visual_parse::parse_visual_edit_patch;
use super::VisualPatchOp;
use crate::tally::{ORIGIN, STEP};
use crate::tool_runtime::shared::{
    jail_write_path, sha256_hex, simple_diff, snapshot_name, snapshot_tag, split_edit_lines,
    string_input,
};
use crate::tool_runtime::ToolRuntime;

/// Line numbers count from one step past none.
const FIRST_LINE: usize = STEP as usize;

/// An ATX heading opener and its level, the run of `#` before the space:
/// https://spec.commonmark.org/0.31.2/#atx-headings
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s").expect("the CommonMark heading pattern is valid"));

fn heading_level(line: &str) -> Option<usize> {
    HEADING
        .captures(line)
        .and_then(|found| found.get(FIRST_LINE))
        .map(|level| level.as_str().len())
}

/// The lines a block anchored at `start_line` spans: a Markdown section up to
/// the next heading of its level or higher, a braced block to its closing
/// brace, or an indented block below a line ending in a colon.
fn visual_block_range(content: &str, start_line: usize) -> Result<(usize, usize), String> {
    let (lines, _) = split_edit_lines(content);
    if start_line < FIRST_LINE || start_line > lines.len() {
        return Err(format!("block start {start_line} is outside the {}-line file", lines.len()));
    }
    let line = &lines[start_line - FIRST_LINE];
    if let Some(level) = heading_level(line) {
        let section = lines
            .iter()
            .skip(start_line)
            .take_while(|text| !heading_level(text).is_some_and(|next| next <= level))
            .count();
        return Ok((start_line, start_line + section));
    }
    if line.contains('{') {
        let mut opened = ORIGIN;
        let mut closed = ORIGIN;
        for (index, text) in lines.iter().enumerate().skip(start_line - FIRST_LINE) {
            opened += text.matches('{').count();
            closed += text.matches('}').count();
            if closed >= opened {
                return Ok((start_line, index + FIRST_LINE));
            }
        }
    }
    if line.trim_end().ends_with(':') {
        let indent = |text: &str| text.chars().take_while(|ch| ch.is_whitespace()).count();
        let base = indent(line);
        let body = lines
            .iter()
            .skip(start_line)
            .take_while(|text| text.trim().is_empty() || indent(text) > base)
            .count();
        if body > ORIGIN {
            return Ok((start_line, start_line + body));
        }
    }
    Err(format!("unsupported block anchor at line {start_line}"))
}

fn anchored_line(op: &VisualPatchOp, name: &str) -> Result<usize, String> {
    op.line.ok_or_else(|| format!("{name} requires line"))
}

fn visual_ops_for_content(content: &str, ops: &[VisualPatchOp]) -> Result<Vec<Value>, String> {
    let (lines, _) = split_edit_lines(content);
    let mut out = Vec::new();
    for op in ops {
        match op.op.as_str() {
            "insert_head" => out.push(json!({"op": "insert_before", "line": FIRST_LINE, "content": op.content})),
            "insert_tail" => out.push(match lines.is_empty() {
                true => json!({"op": "insert_before", "line": FIRST_LINE, "content": op.content}),
                false => json!({"op": "insert_after", "line": lines.len(), "content": op.content}),
            }),
            "replace_block" => {
                let (start, end) = visual_block_range(content, anchored_line(op, "replace_block")?)?;
                out.push(json!({"op": "replace", "startLine": start, "endLine": end, "content": op.content}));
            }
            "delete_block" => {
                let (start, end) = visual_block_range(content, anchored_line(op, "delete_block")?)?;
                out.push(json!({"op": "delete", "startLine": start, "endLine": end}));
            }
            "insert_block_after" => {
                let (_, end) = visual_block_range(content, anchored_line(op, "insert_block_after")?)?;
                out.push(json!({"op": "insert_after", "line": end, "content": op.content}));
            }
            "replace" => out.push(json!({"op": "replace", "startLine": op.start_line, "endLine": op.end_line, "content": op.content})),
            "delete" => out.push(json!({"op": "delete", "startLine": op.start_line, "endLine": op.end_line})),
            "insert_before" | "insert_after" => out.push(json!({"op": op.op, "line": op.line, "content": op.content})),
            _ => return Err(format!("unknown visual op: {}", op.op)),
        }
    }
    Ok(out)
}

/// One file section as the edit will leave it, before anything is written.
pub(super) struct PreparedVisualEdit {
    pub(super) file: PathBuf,
    pub(super) to_file: Option<PathBuf>,
    pub(super) path: String,
    pub(super) to_path: Option<String>,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) ops: Vec<Value>,
    pub(super) remove: bool,
}

pub(crate) fn visual_edit(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("edit requires --allow-write".into());
    }
    let patch = string_input(input, "patch").ok_or("edit requires patch")?;
    let prepared = plan_visual_edit(runtime.cwd, &patch)?;
    commit(&prepared)?;
    Ok(described(&prepared))
}

/// A move destination that already holds another file, or names a file the
/// same patch also edits, refused naming it.
fn checked_destination(
    section_dest: &str,
    to: &Path,
    file: &Path,
    sources: &std::collections::HashSet<PathBuf>,
    destinations: &mut std::collections::HashSet<PathBuf>,
) -> Result<(), String> {
    if !destinations.insert(to.to_path_buf()) {
        return Err(format!("duplicate move destination: {section_dest}"));
    }
    if sources.contains(to) && to != file {
        return Err(format!("move destination is another patched source: {section_dest}"));
    }
    if to != file
        && to
            .try_exists()
            .map_err(|cause| format!("cannot inspect move destination {section_dest}: {cause}"))?
    {
        return Err(format!("destination exists: {section_dest}"));
    }
    Ok(())
}

/// Every section checked against its snapshot and resolved to line operations
/// in original coordinates, writing nothing. Execution and policy hooks share
/// this one reading, so a hook judges exactly what the tool will write.
pub(super) fn plan_visual_edit(cwd: &Path, patch: &str) -> Result<Vec<PreparedVisualEdit>, String> {
    let sections = parse_visual_edit_patch(patch)?;
    let mut source_files = std::collections::HashSet::new();
    for section in &sections {
        let source = jail_write_path(cwd, &section.path)?;
        if !source_files.insert(source) {
            return Err(format!("duplicate patch file section: {}", section.path));
        }
    }
    let mut destination_files = std::collections::HashSet::new();
    let mut prepared = Vec::new();
    for section in &sections {
        let file = jail_write_path(cwd, &section.path)?;
        let current_bytes =
            fs::read(&file).map_err(|cause| format!("cannot read {}: {cause}", file.display()))?;
        let current_tag = snapshot_tag(&sha256_hex(&current_bytes));
        if current_tag != section.tag {
            return Err(format!(
                "snapshot tag mismatch for {}: expected {}, got {}",
                section.path, current_tag, section.tag
            ));
        }
        let to_file = section
            .move_to
            .as_ref()
            .map(|dest| jail_write_path(cwd, dest).map(|to| (dest, to)))
            .transpose()?;
        if let Some((dest, to)) = &to_file {
            checked_destination(dest, to, &file, &source_files, &mut destination_files)?;
        }
        let before = String::from_utf8(current_bytes)
            .map_err(|cause| format!("{} is not UTF-8 text: {cause}", file.display()))?;
        let ops = match section.remove {
            true => Vec::new(),
            false => visual_ops_for_content(&before, &section.ops)?,
        };
        let after = match (section.remove, ops.is_empty()) {
            (true, _) => String::new(),
            (false, true) => before.clone(),
            (false, false) => apply_line_edit_ops(&before, &Value::Array(ops.clone()))?,
        };
        prepared.push(PreparedVisualEdit {
            file,
            to_path: section.move_to.clone(),
            to_file: to_file.map(|(_, to)| to),
            path: section.path.clone(),
            before,
            after,
            ops,
            remove: section.remove,
        });
    }
    Ok(prepared)
}

fn commit(prepared: &[PreparedVisualEdit]) -> Result<(), String> {
    for item in prepared {
        if item.remove {
            fs::remove_file(&item.file)
                .map_err(|cause| format!("cannot delete {}: {cause}", item.file.display()))?;
            continue;
        }
        if !item.ops.is_empty() {
            fs::write(&item.file, item.after.as_bytes())
                .map_err(|cause| format!("cannot write {}: {cause}", item.file.display()))?;
        }
        if let Some(to_file) = &item.to_file {
            if let Some(parent) = to_file.parent() {
                fs::create_dir_all(parent)
                    .map_err(|cause| format!("cannot create {}: {cause}", parent.display()))?;
            }
            fs::rename(&item.file, to_file).map_err(|cause| {
                format!("cannot move {} to {}: {cause}", item.file.display(), to_file.display())
            })?;
        }
    }
    Ok(())
}

fn described(prepared: &[PreparedVisualEdit]) -> Value {
    json!({"files": prepared.iter().map(|item| {
        let final_path = item.to_path.as_ref().unwrap_or(&item.path);
        let next_hash = (!item.remove).then(|| sha256_hex(item.after.as_bytes()));
        let diff = match (item.remove, item.to_path.is_some() && item.ops.is_empty()) {
            (true, _) => simple_diff(&item.path, &item.before, ""),
            (false, true) => format!("rename {} -> {}", item.path, final_path),
            (false, false) => simple_diff(&item.path, &item.before, &item.after),
        };
        json!({
            "path": final_path,
            "from": item.to_path.as_ref().map(|_| item.path.clone()),
            "to": item.to_path,
            "moved": item.to_path.is_some(),
            "deleted": item.remove,
            "sha256": next_hash,
            "snapshot": next_hash.as_ref().map(|hash| snapshot_name(final_path, hash)),
            "diff": diff,
            "ops": item.ops.len(),
            "bytes": item.after.len()
        })
    }).collect::<Vec<_>>()})
}
