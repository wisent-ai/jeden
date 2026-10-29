use crate::tally::STEP;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::shared::{
    amount_input, container_path, count_input, jail_path, string_input, ARCHIVE_SUFFIXES,
    DATABASE_SUFFIXES,
};
use super::ToolRuntime;

mod archive;
mod document;
mod files;
mod sqlite;

pub(crate) use archive::read_archive;
pub(crate) use document::{fetch_readable_url, read_document};
pub(crate) use files::{read_binary_file, read_file, read_image};
pub(crate) use sqlite::read_sqlite;

pub(crate) fn read_any(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read requires path")?;
    if let Some(name) = path.strip_prefix("artifact://") {
        let mut routed = input.clone();
        routed
            .as_object_mut()
            .ok_or("read input must be an object")?
            .insert("name".into(), json!(name));
        return super::session::read_artifact(runtime, &routed);
    }
    if let Some(rest) = path.strip_prefix("mcp://") {
        let (server, uri) = rest
            .split_once('/')
            .ok_or("mcp URI requires mcp://server/resource-uri")?;
        return super::custom::mcp_read_resource(runtime, &json!({"server":server,"uri":uri}));
    }
    if path.starts_with("http://") || path.starts_with("https://") {
        let mut routed = input.clone();
        routed
            .as_object_mut()
            .ok_or("read input must be an object")?
            .insert("url".into(), json!(path));
        return fetch_readable_url(runtime, &routed);
    }
    if let Some((archive, entry)) = container_path(&path, ARCHIVE_SUFFIXES) {
        let mut routed = input.clone();
        let object = routed
            .as_object_mut()
            .ok_or("read input must be an object")?;
        object.insert("path".into(), json!(archive));
        object.insert("entry".into(), json!(entry));
        return read_archive(runtime, &routed);
    }
    if let Some((database, selector)) = container_path(&path, DATABASE_SUFFIXES) {
        let mut routed = input.clone();
        let object = routed
            .as_object_mut()
            .ok_or("read input must be an object")?;
        object.insert("path".into(), json!(database));
        match selector.split_once(':') {
            Some((table, key)) => {
                object.insert("table".into(), json!(table));
                object.insert("key".into(), json!(key));
            }
            None => {
                object.insert("table".into(), json!(selector));
            }
        }
        return read_sqlite(runtime, &routed);
    }
    // A `:` after the file name selects lines or an entry; the kind is the file's.
    let plain = match path.split_once(':') {
        Some((file, _selector)) => file.to_ascii_lowercase(),
        None => path.to_ascii_lowercase(),
    };
    if plain.ends_with(".ipynb")
        || plain.ends_with(".pdf")
        || plain.ends_with(".csv")
        || plain.ends_with(".html")
        || plain.ends_with(".xml")
    {
        return read_document(runtime, input);
    }
    read_file(runtime, input)
}

/// Lists `root` into `entries`, and the directories under it while `depth`
/// levels below are left, until `limit` entries are held.
fn walk_dir(
    runtime: &ToolRuntime<'_>,
    root: &Path,
    depth: usize,
    limit: usize,
    entries: &mut Vec<Value>,
) -> Result<(), String> {
    if runtime.operation.cancellation().is_cancelled() {
        return Err("directory read cancelled".into());
    }
    if entries.len() >= limit {
        return Ok(());
    }
    let unlisted = |error: std::io::Error| format!("cannot list {}: {error}", root.display());
    let mut children = fs::read_dir(root)
        .map_err(unlisted)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(unlisted)?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if entries.len() >= limit {
            break;
        }
        if runtime.operation.cancellation().is_cancelled() {
            return Err("directory read cancelled".into());
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let kind = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        let shown = match path.strip_prefix(runtime.cwd) {
            Ok(inside) => inside,
            Err(_outside) => path.as_path(),
        };
        // Only a file has a size; any other entry answers none.
        let size = metadata.is_file().then_some(metadata.len());
        entries.push(json!({"name": entry.file_name().to_string_lossy(), "path": shown.to_string_lossy().replace('\\', "/"), "type": kind, "size": size}));
        if metadata.is_dir() {
            if let Some(deeper) = depth.checked_sub(usize::from(STEP)) {
                walk_dir(runtime, &path, deeper, limit, entries)?;
            }
        }
    }
    Ok(())
}

/// `list_dir`: the entries of `path` (the workspace root when none is
/// stated), descending `depth` levels (none when it is not stated).
pub(crate) fn list_dir(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = match string_input(input, "path") {
        Some(path) => path,
        None => ".".to_owned(),
    };
    let limit = count_input(input, "limit", "list_dir")? as usize;
    let depth = amount_input(input, "depth", "list_dir")?.0;
    let dir = jail_path(runtime.cwd, &path)?;
    match std::fs::metadata(&dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_not_directory) => return Err(format!("not a directory: {path}")),
        Err(error) => return Err(format!("{path} cannot be examined: {error}")),
    }
    let mut entries = Vec::new();
    walk_dir(runtime, &dir, depth, limit, &mut entries)?;
    Ok(
        json!({"ok":true,"path":path,"depth":depth,"limit":limit,"truncated":entries.len()==limit,"entries":entries}),
    )
}

/// A PDF file's own first bytes: the file header the PDF specification
/// requires every PDF to begin with.
pub(crate) const PDF_SIGNATURE: &[u8] = b"%PDF-";
pub(crate) const PDF_TYPE: &str = "application/pdf";
pub(crate) const TEXT_TYPE: &str = "text/plain";
pub(crate) const BINARY_TYPE: &str = "application/octet-stream";

/// The media type an image format's own signature names, when `bytes` start
/// with one (see `files::image_dimensions`); `shared::mime_type_of` asks it.
pub(crate) fn image_signature(bytes: &[u8]) -> Option<&'static str> {
    files::image_dimensions(bytes).map(|(_, _, mime_type)| mime_type)
}
