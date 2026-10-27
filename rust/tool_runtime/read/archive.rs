//! Reading one entry of a zip or tar archive, or listing them all, through
//! the `zip` and `tar` crates the write side uses too.

use base64::{engine::general_purpose, Engine as _};
use flate2::read::GzDecoder;
use serde_json::{json, Value};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::document::{answered_text, readable_text_for_document};
use super::files::image_dimensions;
use crate::tool_runtime::shared::{
    count_input, jail_path, mime_type_of, selected_lines, sha256_hex, string_input,
};
use crate::tool_runtime::ToolRuntime;

/// One archive entry. Only the entry a read asked for carries its bytes.
struct ArchiveEntry {
    name: String,
    size: u64,
    directory: bool,
    content: Vec<u8>,
}

enum Format {
    Zip,
    Tar,
    TarGz,
}

fn format_of(path: &Path) -> Result<Format, String> {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".zip") {
        Ok(Format::Zip)
    } else if lower.ends_with(".tgz") || lower.ends_with(".tar.gz") {
        Ok(Format::TarGz)
    } else if lower.ends_with(".tar") {
        Ok(Format::Tar)
    } else {
        Err(format!(
            "{} is not a supported archive: .zip, .tar, .tar.gz, .tgz",
            path.display()
        ))
    }
}

fn archive_entries(path: &Path, wanted: Option<&str>) -> Result<Vec<ArchiveEntry>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let unreadable = |error: String| format!("{} cannot be read as an archive: {error}", path.display());
    match format_of(path)? {
        Format::Zip => zip_entries(file, wanted).map_err(unreadable),
        Format::Tar => tar_entries(file, wanted).map_err(unreadable),
        Format::TarGz => tar_entries(GzDecoder::new(file), wanted).map_err(unreadable),
    }
}

fn zip_entries(file: File, wanted: Option<&str>) -> Result<Vec<ArchiveEntry>, String> {
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let mut entries = Vec::new();
    for name in names {
        let mut entry = archive
            .by_name(&name)
            .map_err(|error| format!("entry {name}: {error}"))?;
        let mut content = Vec::new();
        if wanted == Some(name.as_str()) {
            entry
                .read_to_end(&mut content)
                .map_err(|error| format!("entry {name}: {error}"))?;
        }
        entries.push(ArchiveEntry {
            size: entry.size(),
            directory: entry.is_dir(),
            name,
            content,
        });
    }
    Ok(entries)
}

fn tar_entries(reader: impl Read, wanted: Option<&str>) -> Result<Vec<ArchiveEntry>, String> {
    let mut archive = tar::Archive::new(reader);
    let mut entries = Vec::new();
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        let mut content = Vec::new();
        if wanted == Some(name.as_str()) {
            entry
                .read_to_end(&mut content)
                .map_err(|error| format!("entry {name}: {error}"))?;
        }
        entries.push(ArchiveEntry {
            size: entry.size(),
            directory: entry.header().entry_type().is_dir(),
            name,
            content,
        });
    }
    Ok(entries)
}

pub(crate) fn read_archive(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_archive requires path")?;
    let file = jail_path(runtime.cwd, &path)?;
    let wanted = string_input(input, "entry");
    let entries = archive_entries(&file, wanted.as_deref())?;
    let Some(entry_name) = wanted else {
        let listed = entries
            .iter()
            .map(|entry| {
                json!({"name": entry.name, "type": if entry.directory { "dir" } else { "file" }, "bytes": entry.size})
            })
            .collect::<Vec<_>>();
        return Ok(json!({"path": path, "entries": listed}));
    };
    let entry = entries
        .into_iter()
        .find(|entry| entry.name == entry_name)
        .ok_or_else(|| format!("archive entry not found: {entry_name}"))?;
    if entry.directory {
        return Err(format!("archive entry is not a file: {entry_name}"));
    }
    let max_bytes = count_input(input, "maxBytes", "read_archive")? as usize;
    // An entry is read as text unless the caller names another mode.
    let mode = match string_input(input, "mode") {
        Some(mode) => mode,
        None => "text".to_owned(),
    };
    let sliced = &entry.content[..entry.content.len().min(max_bytes)];
    let truncated = entry.content.len() > sliced.len();
    match mode.as_str() {
        "binary" => Ok(
            json!({"path": path, "entry": entry.name, "mode": mode, "bytes": entry.content.len(), "truncated": truncated, "mimeType": mime_type_of(&entry.content), "base64": general_purpose::STANDARD.encode(sliced), "sha256": sha256_hex(&entry.content)}),
        ),
        "image" => {
            let Some((width, height, mime_type)) = image_dimensions(&entry.content) else {
                return Err(format!(
                    "archive entry {entry_name} is not an image this reader knows (PNG, JPEG, GIF, WebP)"
                ));
            };
            Ok(
                json!({"path": path, "entry": entry.name, "mode": mode, "bytes": entry.content.len(), "truncated": truncated, "mimeType": mime_type, "width": width, "height": height, "base64": general_purpose::STANDARD.encode(sliced), "sha256": sha256_hex(&entry.content)}),
            )
        }
        "document" => {
            let text = readable_text_for_document(&entry.content)?;
            let window = selected_lines(&text, string_input(input, "range"))?;
            let (answered, truncated) = answered_text(&text, window.as_ref(), max_bytes);
            Ok(
                json!({"path": path, "entry": entry.name, "mode": mode, "bytes": text.len(), "truncated": truncated, "sha256": sha256_hex(&entry.content), "text": answered, "mimeType": mime_type_of(&entry.content), "startLine": window.as_ref().map(|window| window.start_line), "endLine": window.as_ref().map(|window| window.end_line), "ranges": window.as_ref().map(|window| &window.ranges)}),
            )
        }
        "text" => {
            let text = String::from_utf8_lossy(&entry.content).into_owned();
            let window = selected_lines(&text, string_input(input, "range"))?;
            let (answered, truncated) = answered_text(&text, window.as_ref(), max_bytes);
            Ok(
                json!({"path": path, "entry": entry.name, "mode": mode, "bytes": entry.content.len(), "truncated": truncated, "sha256": sha256_hex(&entry.content), "content": answered, "startLine": window.as_ref().map(|window| window.start_line), "endLine": window.as_ref().map(|window| window.end_line), "ranges": window.as_ref().map(|window| &window.ranges)}),
            )
        }
        other => Err(format!(
            "unsupported archive read mode {other:?}: text, document, binary or image"
        )),
    }
}
