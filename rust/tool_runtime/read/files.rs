use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use std::fs;

use crate::tool_runtime::shared::{jail_path, mime_type_for_path, sha256_hex, string_input, u64_input, MAX_READ_BYTES};
use crate::tool_runtime::ToolRuntime;

pub(crate) fn read_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_file requires path")?;
    let file = jail_path(runtime.cwd, &path)?;
    let meta = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !meta.is_file() { return Err(format!("not a file: {path}")); }
    if meta.len() > MAX_READ_BYTES { return Err(format!("file too large: {} bytes", meta.len())); }
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let content = String::from_utf8(bytes.clone()).map_err(|_| format!("file is not UTF-8: {path}"))?;
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "sha256": sha256_hex(&bytes), "content": content}))
}

pub(crate) fn read_binary_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_binary_file requires path")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let meta = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !meta.is_file() { return Err(format!("not a file: {path}")); }
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "truncated": truncated, "mimeType": mime_type_for_path(&file), "sha256": sha256_hex(&bytes), "base64": general_purpose::STANDARD.encode(slice)}))
}

pub(super) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32, &'static str)> {
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((width, height, "image/png"));
    }
    if bytes.len() >= 10 && &bytes[0..3] == b"GIF" {
        let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Some((width, height, "image/gif"));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some((0, 0, "image/webp"));
    }
    if bytes.len() > 4 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xff { i += 1; continue; }
            let marker = bytes[i + 1];
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if matches!(marker, 0xc0 | 0xc1 | 0xc2 | 0xc3) && i + 8 < bytes.len() {
                let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((width, height, "image/jpeg"));
            }
            if len < 2 { break; }
            i += 2 + len;
        }
        return Some((0, 0, "image/jpeg"));
    }
    None
}

pub(crate) fn read_image(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_image requires path")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let Some((width, height, mime_type)) = image_dimensions(&bytes) else {
        return Err(format!("unsupported image type: {}", mime_type_for_path(&file)));
    };
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "truncated": truncated, "mimeType": mime_type, "width": width, "height": height, "base64": general_purpose::STANDARD.encode(slice), "sha256": sha256_hex(&bytes)}))
}
