use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::shared::{jail_path, string_input, u64_input};
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
        routed.as_object_mut().ok_or("read input must be an object")?.insert("name".into(), json!(name));
        return super::session::read_artifact(runtime, &routed);
    }
    if let Some(rest) = path.strip_prefix("mcp://") {
        let (server, uri) = rest.split_once('/').ok_or("mcp URI requires mcp://server/resource-uri")?;
        return super::custom::mcp_read_resource(runtime, &json!({"server":server,"uri":uri,"timeoutMs":u64_input(input,"timeoutMs",30_000)}));
    }
    if path.starts_with("http://") || path.starts_with("https://") {
        let mut routed=input.clone(); routed.as_object_mut().ok_or("read input must be an object")?.insert("url".into(),json!(path));
        return fetch_readable_url(runtime,&routed);
    }
    let lower=path.to_ascii_lowercase();
    for suffix in [".tar.gz:",".tgz:",".tar:",".zip:"] {
        if let Some(index)=lower.find(suffix) { let split=index+suffix.len()-1; let mut routed=input.clone();let object=routed.as_object_mut().ok_or("read input must be an object")?;object.insert("path".into(),json!(&path[..split]));object.insert("entry".into(),json!(&path[split+1..]));return read_archive(runtime,&routed); }
    }
    if let Some(index)=lower.find(".sqlite:").or_else(||lower.find(".db:")) {
        let extension=if lower[index..].starts_with(".sqlite:"){8}else{4};let split=index+extension-1;let selector=&path[split+1..];let mut routed=input.clone();let object=routed.as_object_mut().ok_or("read input must be an object")?;object.insert("path".into(),json!(&path[..split]));
        let (table,key)=selector.split_once(':').map_or((selector,None),|(table,key)|(table,Some(key)));object.insert("table".into(),json!(table));if let Some(key)=key{object.insert("key".into(),json!(key));}return read_sqlite(runtime,&routed);
    }
    let plain=path.split(':').next().unwrap_or(&path).to_ascii_lowercase();
    if plain.ends_with(".ipynb") || plain.ends_with(".pdf") || plain.ends_with(".csv") || plain.ends_with(".html") || plain.ends_with(".xml") { return read_document(runtime,input); }
    read_file(runtime,input)
}

fn walk_dir(runtime: &ToolRuntime<'_>, root: &Path, depth: usize, limit: usize, entries: &mut Vec<Value>) -> Result<(), String> {
    if runtime.operation.cancellation().is_cancelled() { return Err("directory read cancelled".into()); }
    if entries.len() >= limit { return Ok(()); }
    let mut children = fs::read_dir(root).map_err(|error| error.to_string())?.filter_map(Result::ok).collect::<Vec<_>>();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if entries.len() >= limit { break; }
        if runtime.operation.cancellation().is_cancelled() { return Err("directory read cancelled".into()); }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        let kind = if metadata.file_type().is_symlink() { "symlink" } else if metadata.is_dir() { "directory" } else if metadata.is_file() { "file" } else { "other" };
        entries.push(json!({"name":entry.file_name().to_string_lossy(),"path":path.strip_prefix(runtime.cwd).unwrap_or(&path).to_string_lossy().replace('\\',"/"),"type":kind,"size":if metadata.is_file(){metadata.len()}else{0}}));
        if metadata.is_dir() && depth > 0 { walk_dir(runtime, &path, depth - 1, limit, entries)?; }
    }
    Ok(())
}

pub(crate) fn list_dir(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").unwrap_or_else(|| ".".into());
    let limit = u64_input(input, "limit", 200).clamp(1, 10_000) as usize;
    let depth = u64_input(input, "depth", 0).min(64) as usize;
    let dir = jail_path(runtime.cwd, &path)?;
    if !dir.is_dir() { return Err(format!("not a directory: {path}")); }
    let mut entries = Vec::new();
    walk_dir(runtime, &dir, depth, limit, &mut entries)?;
    Ok(json!({"ok":true,"path":path,"depth":depth,"limit":limit,"truncated":entries.len()==limit,"entries":entries}))
}
