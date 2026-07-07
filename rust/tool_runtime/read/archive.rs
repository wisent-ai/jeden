use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::tool_runtime::shared::{jail_path, line_window, mime_type_for_path, sha256_hex, string_input, u64_input, MAX_READ_BYTES};
use crate::tool_runtime::ToolRuntime;
use super::document::readable_text_for_document;
use super::files::image_dimensions;

struct ArchiveEntry {
    name: String,
    size: usize,
    kind: &'static str,
    content: Vec<u8>,
}

fn tar_entries(bytes: &[u8]) -> Vec<ArchiveEntry> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) { break; }
        let read_string = |range: std::ops::Range<usize>| -> String {
            String::from_utf8_lossy(&header[range]).trim_end_matches('\0').to_string()
        };
        let name = read_string(0..100);
        let prefix = read_string(345..500);
        let size_text = read_string(124..136).trim().to_string();
        let size = usize::from_str_radix(if size_text.is_empty() { "0" } else { &size_text }, 8).unwrap_or(0);
        let type_flag = header[156] as char;
        let full_name = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let data_start = offset + 512;
        let data_end = data_start.saturating_add(size).min(bytes.len());
        if !full_name.is_empty() {
            entries.push(ArchiveEntry {
                name: full_name,
                size,
                kind: if type_flag == '5' { "dir" } else { "file" },
                content: bytes[data_start..data_end].to_vec(),
            });
        }
        offset = data_start.saturating_add(size.div_ceil(512) * 512);
    }
    entries
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes.get(offset..offset + 2).ok_or("truncated zip")?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes.get(offset..offset + 4).ok_or("truncated zip")?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn zip_entries(bytes: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    let mut eocd = None;
    let mut i = bytes.len().saturating_sub(22);
    loop {
        if le_u32(bytes, i).ok() == Some(0x06054b50) { eocd = Some(i); break; }
        if i == 0 { break; }
        i -= 1;
    }
    let eocd = eocd.ok_or("zip central directory not found")?;
    let total = le_u16(bytes, eocd + 10)? as usize;
    let mut offset = le_u32(bytes, eocd + 16)? as usize;
    let mut entries = Vec::new();
    for _ in 0..total {
        if le_u32(bytes, offset)? != 0x02014b50 { return Err("invalid zip central directory".into()); }
        let method = le_u16(bytes, offset + 10)?;
        let compressed_size = le_u32(bytes, offset + 20)? as usize;
        let size = le_u32(bytes, offset + 24)? as usize;
        let name_length = le_u16(bytes, offset + 28)? as usize;
        let extra_length = le_u16(bytes, offset + 30)? as usize;
        let comment_length = le_u16(bytes, offset + 32)? as usize;
        let local_offset = le_u32(bytes, offset + 42)? as usize;
        let name_bytes = bytes.get(offset + 46..offset + 46 + name_length).ok_or("truncated zip filename")?;
        let name = String::from_utf8_lossy(name_bytes).to_string();
        let local_name_length = le_u16(bytes, local_offset + 26)? as usize;
        let local_extra_length = le_u16(bytes, local_offset + 28)? as usize;
        let data_start = local_offset + 30 + local_name_length + local_extra_length;
        let compressed = bytes.get(data_start..data_start + compressed_size).ok_or("truncated zip entry")?;
        let is_dir = name.ends_with('/');
        let content = if is_dir {
            Vec::new()
        } else if method == 0 {
            compressed.to_vec()
        } else if method == 8 {
            let mut decoder = flate2::read::DeflateDecoder::new(compressed);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
            out
        } else {
            return Err(format!("unsupported zip compression method: {method}"));
        };
        entries.push(ArchiveEntry { name, size, kind: if is_dir { "dir" } else { "file" }, content });
        offset += 46 + name_length + extra_length + comment_length;
    }
    Ok(entries)
}

fn archive_entries(path: &Path, bytes: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".zip") { return zip_entries(bytes); }
    if lower.ends_with(".tgz") || lower.ends_with(".tar.gz") {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
        return Ok(tar_entries(&out));
    }
    if lower.ends_with(".tar") { return Ok(tar_entries(bytes)); }
    Err("supported archives: .zip, .tar, .tar.gz, .tgz".into())
}

pub(crate) fn read_archive(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_archive requires path")?;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let entries = archive_entries(&file, &bytes)?;
    if string_input(input, "entry").is_none() {
        let listed = entries.iter().map(|entry| json!({"name": entry.name, "type": entry.kind, "bytes": entry.size})).collect::<Vec<_>>();
        return Ok(json!({"path": path, "entries": listed}));
    }
    let entry_name = string_input(input, "entry").unwrap();
    let entry = entries.into_iter().find(|entry| entry.name == entry_name).ok_or_else(|| format!("archive entry not found: {entry_name}"))?;
    if entry.kind != "file" { return Err(format!("archive entry is not a file: {entry_name}")); }
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).clamp(1, MAX_READ_BYTES) as usize;
    let mode = string_input(input, "mode").unwrap_or_else(|| "text".into());
    let sliced = &entry.content[..entry.content.len().min(max_bytes)];
    if mode == "binary" {
        return Ok(json!({"path": path, "entry": entry.name, "mode": mode, "bytes": entry.content.len(), "truncated": entry.content.len() > sliced.len(), "mimeType": mime_type_for_path(Path::new(&entry.name)), "base64": general_purpose::STANDARD.encode(sliced), "sha256": sha256_hex(&entry.content)}));
    }
    if mode == "image" {
        let Some((width, height, mime_type)) = image_dimensions(&entry.content) else {
            return Err(format!("unsupported image type: {}", mime_type_for_path(Path::new(&entry.name))));
        };
        return Ok(json!({"path": path, "entry": entry.name, "mode": mode, "bytes": entry.content.len(), "truncated": entry.content.len() > sliced.len(), "mimeType": mime_type, "width": width, "height": height, "base64": general_purpose::STANDARD.encode(sliced), "sha256": sha256_hex(&entry.content)}));
    }
    let text = if mode == "document" {
        readable_text_for_document(&entry.content, Path::new(&entry.name), None)?
    } else if mode == "text" {
        String::from_utf8_lossy(&entry.content).to_string()
    } else {
        return Err(format!("unsupported archive read mode: {mode}"));
    };
    let selected = if let Some(range) = string_input(input, "range") { line_window(&text, &range)? } else { (text.clone(), 0, 0, Vec::new()) };
    let output_bytes = selected.0.as_bytes();
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    let key = if mode == "document" { "text" } else { "content" };
    let mut out = json!({"path": path, "entry": entry.name, "mode": mode, "bytes": if mode == "document" { text.len() } else { entry.content.len() }, "truncated": output_bytes.len() > slice.len(), "sha256": sha256_hex(&entry.content), "startLine": if selected.3.is_empty() { Value::Null } else { json!(selected.1) }, "endLine": if selected.3.is_empty() { Value::Null } else { json!(selected.2) }, "ranges": if selected.3.is_empty() { Value::Null } else { json!(selected.3) }});
    out[key] = json!(String::from_utf8_lossy(slice));
    if mode == "document" { out["mimeType"] = json!(mime_type_for_path(Path::new(&entry.name))); }
    Ok(out)
}
