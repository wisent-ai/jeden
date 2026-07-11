use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use crate::tool_runtime::shared::{
    jail_path, mime_type_for_path, sha256_hex, snapshot_name, string_input, u64_input,
    MAX_READ_BYTES,
};
use crate::tool_runtime::ToolRuntime;

#[derive(Clone, Copy)]
struct RequestedRange {
    start: usize,
    end: Option<usize>,
}

enum Selector {
    Lines(Vec<RequestedRange>),
    Conflicts,
    Raw,
}

fn path_and_selector(
    runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<(String, std::path::PathBuf, Option<String>), String> {
    let raw_path = string_input(input, "path").ok_or("read_file requires path")?;
    let explicit = string_input(input, "selector").or_else(|| string_input(input, "range"));
    if explicit.is_some() {
        return Ok((
            raw_path.clone(),
            jail_path(runtime.cwd, &raw_path)?,
            explicit,
        ));
    }
    if let Some((candidate, suffix)) = raw_path.rsplit_once(':') {
        if selector(suffix).is_ok() {
            if let Ok(path) = jail_path(runtime.cwd, candidate) {
                if path.is_file() {
                    return Ok((candidate.to_string(), path, Some(suffix.to_string())));
                }
            }
        }
    }
    Ok((raw_path.clone(), jail_path(runtime.cwd, &raw_path)?, None))
}

fn selector(raw: &str) -> Result<Selector, String> {
    match raw {
        "raw" => return Ok(Selector::Raw),
        "conflicts" => return Ok(Selector::Conflicts),
        _ => {}
    }
    let mut ranges = Vec::new();
    for item in raw.split(',') {
        let item = item.trim();
        let item = item.replace(':', "-");
        let item = item.as_str();
        let (start, end) = if let Some((left, count)) = item.split_once('+') {
            let start = left
                .parse::<usize>()
                .map_err(|_| format!("invalid selector: {raw}"))?;
            let count = count
                .parse::<usize>()
                .map_err(|_| format!("invalid selector: {raw}"))?;
            (start, Some(start.saturating_add(count).saturating_sub(1)))
        } else if let Some((left, right)) = item.split_once('-') {
            (
                left.parse::<usize>()
                    .map_err(|_| format!("invalid selector: {raw}"))?,
                if right.is_empty() {
                    None
                } else {
                    Some(
                        right
                            .parse::<usize>()
                            .map_err(|_| format!("invalid selector: {raw}"))?,
                    )
                },
            )
        } else {
            let line = item
                .parse::<usize>()
                .map_err(|_| format!("invalid selector: {raw}"))?;
            (line, Some(line))
        };
        if start == 0 || end.is_some_and(|end| end < start) {
            return Err(format!("invalid selector: {raw}"));
        }
        ranges.push(RequestedRange { start, end });
    }
    if ranges.is_empty() {
        return Err(format!("invalid selector: {raw}"));
    }
    Ok(Selector::Lines(ranges))
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn ranged_text(
    runtime: &ToolRuntime<'_>,
    path: &Path,
    selected: Selector,
) -> Result<(String, Vec<Value>, usize, bool), String> {
    let mut reader = BufReader::new(File::open(path).map_err(|error| error.to_string())?);
    let mut line = String::new();
    let mut content = String::new();
    let mut visual = Vec::new();
    let mut line_number = 0usize;
    let mut conflict = false;
    let mut truncated = false;
    loop {
        if runtime.operation.cancellation().is_cancelled() {
            return Err("read cancelled".into());
        }
        line.clear();
        if reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            break;
        }
        line_number += 1;
        let text = line
            .strip_suffix('\n')
            .unwrap_or(&line)
            .strip_suffix('\r')
            .unwrap_or(line.strip_suffix('\n').unwrap_or(&line));
        let include = match &selected {
            Selector::Raw => true,
            Selector::Lines(ranges) => ranges.iter().any(|range| {
                line_number >= range.start && range.end.is_none_or(|end| line_number <= end)
            }),
            Selector::Conflicts => {
                if text.starts_with("<<<<<<< ") {
                    conflict = true;
                }
                let included = conflict;
                if text.starts_with(">>>>>>> ") {
                    conflict = false;
                }
                included
            }
        };
        if !include {
            continue;
        }
        let needed = text.len() + usize::from(!content.is_empty());
        if content.len().saturating_add(needed) > MAX_READ_BYTES as usize {
            truncated = true;
            break;
        }
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(text);
        visual.push(json!({"line":line_number,"text":text}));
    }
    Ok((content, visual, line_number, truncated))
}

pub(crate) fn read_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let (label, file, raw_selector) = path_and_selector(runtime, input)?;
    let meta = fs::metadata(&file).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err(format!("not a file: {label}"));
    }
    if raw_selector.is_none() && meta.len() > MAX_READ_BYTES {
        return Err(format!(
            "file too large: {} bytes; provide a selector",
            meta.len()
        ));
    }
    let selected = raw_selector
        .as_deref()
        .map(selector)
        .transpose()?
        .unwrap_or(Selector::Raw);
    let (content, lines, scanned_lines, truncated) = ranged_text(runtime, &file, selected)?;
    let sha256 = hash_file(&file)?;
    std::str::from_utf8(content.as_bytes()).map_err(|_| format!("file is not UTF-8: {label}"))?;
    Ok(
        json!({"ok":true,"path":label,"snapshot":snapshot_name(&label,&sha256),"bytes":meta.len(),"sha256":sha256,"selector":raw_selector,"content":content,"lines":lines,"scannedLines":scanned_lines,"truncated":truncated}),
    )
}

pub(crate) fn read_binary_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_binary_file requires path")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let meta = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("not a file: {path}"));
    }
    let mut reader = File::open(&file).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(max_bytes.min(meta.len() as usize));
    reader
        .by_ref()
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(
        json!({"ok":true,"path":path,"bytes":meta.len(),"truncated":meta.len()>max_bytes as u64,"mimeType":mime_type_for_path(&file),"sha256":hash_file(&file)?,"base64":general_purpose::STANDARD.encode(bytes)}),
    )
}

pub(super) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32, &'static str)> {
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        return Some((
            u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            "image/png",
        ));
    }
    if bytes.len() >= 10 && &bytes[0..3] == b"GIF" {
        return Some((
            u16::from_le_bytes([bytes[6], bytes[7]]) as u32,
            u16::from_le_bytes([bytes[8], bytes[9]]) as u32,
            "image/gif",
        ));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some((0, 0, "image/webp"));
    }
    if bytes.len() > 4 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        let mut index = 2usize;
        while index + 9 < bytes.len() {
            if bytes[index] != 0xff {
                index += 1;
                continue;
            }
            let marker = bytes[index + 1];
            let len = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
            if matches!(marker, 0xc0 | 0xc1 | 0xc2 | 0xc3) {
                return Some((
                    u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32,
                    u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32,
                    "image/jpeg",
                ));
            }
            if len < 2 {
                break;
            }
            index += 2 + len;
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
        return Err(format!(
            "unsupported image type: {}",
            mime_type_for_path(&file)
        ));
    };
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(
        json!({"ok":true,"path":path,"bytes":bytes.len(),"truncated":bytes.len()>max_bytes,"mimeType":mime_type,"width":width,"height":height,"base64":general_purpose::STANDARD.encode(slice),"sha256":sha256_hex(&bytes)}),
    )
}
