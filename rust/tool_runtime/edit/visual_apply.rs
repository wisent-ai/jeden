use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use super::lines::apply_line_edit_ops;
use super::visual_parse::parse_visual_edit_patch;
use super::VisualPatchOp;
use crate::tool_runtime::shared::{
    jail_path, sha256_hex, simple_diff, snapshot_name, snapshot_tag, split_edit_lines, string_input,
};
use crate::tool_runtime::ToolRuntime;

fn visual_block_range(content: &str, start_line: usize) -> Result<(usize, usize), String> {
    let (lines, _) = split_edit_lines(content);
    if start_line < 1 || start_line > lines.len() {
        return Err("block start is past end of file".into());
    }
    let line = &lines[start_line - 1];
    if let Some(heading) = regex::Regex::new(r"^(#{1,6})\s")
        .ok()
        .and_then(|re| re.captures(line))
    {
        let level = heading.get(1).map(|m| m.as_str().len()).unwrap_or(1);
        let heading_re = regex::Regex::new(r"^(#{1,6})\s").unwrap();
        for index in start_line..lines.len() {
            if let Some(next) = heading_re.captures(&lines[index]) {
                if next.get(1).map(|m| m.as_str().len()).unwrap_or(7) <= level {
                    return Ok((start_line, index));
                }
            }
        }
        return Ok((start_line, lines.len()));
    }
    if line.contains('{') {
        let mut balance = 0i64;
        let mut opened = false;
        for (idx, text) in lines.iter().enumerate().skip(start_line - 1) {
            for ch in text.chars() {
                if ch == '{' {
                    balance += 1;
                    opened = true;
                }
                if ch == '}' {
                    balance -= 1;
                }
            }
            if opened && balance <= 0 {
                return Ok((start_line, idx + 1));
            }
        }
    }
    if line.trim_end().ends_with(':') {
        let base_indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
        let mut end_line = start_line;
        for (idx, text) in lines.iter().enumerate().skip(start_line) {
            if text.trim().is_empty() {
                end_line = idx + 1;
                continue;
            }
            let indent = text.chars().take_while(|ch| ch.is_whitespace()).count();
            if indent <= base_indent {
                break;
            }
            end_line = idx + 1;
        }
        if end_line > start_line {
            return Ok((start_line, end_line));
        }
    }
    Err(format!("unsupported block anchor at line {start_line}"))
}

fn visual_ops_for_content(content: &str, ops: &[VisualPatchOp]) -> Result<Vec<Value>, String> {
    let (lines, _) = split_edit_lines(content);
    let mut out = Vec::new();
    for op in ops {
        match op.op.as_str() {
            "insert_head" => out.push(json!({"op": "insert_before", "line": 1, "content": op.content})),
            "insert_tail" => out.push(if lines.is_empty() { json!({"op": "insert_before", "line": 1, "content": op.content}) } else { json!({"op": "insert_after", "line": lines.len(), "content": op.content}) }),
            "replace_block" => {
                let (start, end) = visual_block_range(content, op.line.ok_or("replace_block requires line")?)?;
                out.push(json!({"op": "replace", "startLine": start, "endLine": end, "content": op.content}));
            }
            "delete_block" => {
                let (start, end) = visual_block_range(content, op.line.ok_or("delete_block requires line")?)?;
                out.push(json!({"op": "delete", "startLine": start, "endLine": end}));
            }
            "insert_block_after" => {
                let (_, end) = visual_block_range(content, op.line.ok_or("insert_block_after requires line")?)?;
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

struct PreparedVisualEdit {
    file: PathBuf,
    to_file: Option<PathBuf>,
    path: String,
    to_path: Option<String>,
    before: String,
    after: String,
    ops: Vec<Value>,
    remove: bool,
}

pub(crate) fn visual_edit(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("edit requires --allow-write".into());
    }
    let patch = string_input(input, "patch").ok_or("edit requires patch")?;
    let sections = parse_visual_edit_patch(&patch)?;
    let mut source_files = std::collections::HashSet::new();
    for section in &sections {
        let source = jail_path(runtime.cwd, &section.path)?;
        if !source_files.insert(source) {
            return Err(format!("duplicate patch file section: {}", section.path));
        }
    }
    let mut destination_files = std::collections::HashSet::new();
    let mut prepared = Vec::new();
    for section in &sections {
        let file = jail_path(runtime.cwd, &section.path)?;
        let current_bytes = fs::read(&file).map_err(|e| e.to_string())?;
        let current_hash = sha256_hex(&current_bytes);
        let current_tag = snapshot_tag(&current_hash);
        if current_tag != section.tag {
            return Err(format!(
                "snapshot tag mismatch for {}: expected {}, got {}",
                section.path, current_tag, section.tag
            ));
        }
        let to_file = section
            .move_to
            .as_ref()
            .map(|dest| jail_path(runtime.cwd, dest))
            .transpose()?;
        if let Some(to) = &to_file {
            if !destination_files.insert(to.clone()) {
                return Err(format!(
                    "duplicate move destination: {}",
                    section.move_to.as_deref().unwrap_or("")
                ));
            }
            if source_files.contains(to) && to != &file {
                return Err(format!(
                    "move destination is another patched source: {}",
                    section.move_to.as_deref().unwrap_or("")
                ));
            }
            if to != &file && to.exists() {
                return Err(format!(
                    "destination exists: {}",
                    section.move_to.as_deref().unwrap_or("")
                ));
            }
        }
        let before = String::from_utf8(current_bytes).map_err(|e| e.to_string())?;
        let ops = if section.remove {
            Vec::new()
        } else {
            visual_ops_for_content(&before, &section.ops)?
        };
        let after = if section.remove {
            String::new()
        } else if ops.is_empty() {
            before.clone()
        } else {
            apply_line_edit_ops(&before, &Value::Array(ops.clone()))?
        };
        prepared.push(PreparedVisualEdit {
            file,
            to_path: section.move_to.clone(),
            to_file,
            path: section.path.clone(),
            before,
            after,
            ops,
            remove: section.remove,
        });
    }
    for item in &prepared {
        if item.remove {
            fs::remove_file(&item.file).map_err(|e| e.to_string())?;
        } else {
            if !item.ops.is_empty() {
                fs::write(&item.file, item.after.as_bytes()).map_err(|e| e.to_string())?;
            }
            if let Some(to_file) = &item.to_file {
                if let Some(parent) = to_file.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::rename(&item.file, to_file).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(json!({"files": prepared.iter().map(|item| {
        let final_path = item.to_path.as_ref().unwrap_or(&item.path);
        let next_hash = if item.remove { None } else { Some(sha256_hex(item.after.as_bytes())) };
        let diff = if item.remove {
            simple_diff(&item.path, &item.before, "")
        } else if item.to_path.is_some() && item.ops.is_empty() {
            format!("rename {} -> {}", item.path, final_path)
        } else {
            simple_diff(&item.path, &item.before, &item.after)
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
    }).collect::<Vec<_>>()}))
}
