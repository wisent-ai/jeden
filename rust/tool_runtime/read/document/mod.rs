//! Reading a document off disk or off the network as text a model can use.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::tool_runtime::shared::{
    jail_path, line_window, mime_type_for_path, sha256_hex, string_input, u64_input, MAX_READ_BYTES,
};
use crate::tool_runtime::ToolRuntime;

mod formats;
mod tables;

use formats::{
    readable_text_from_feed, readable_text_from_html, readable_text_from_json,
    readable_text_from_notebook, readable_text_from_pdf,
};
use tables::readable_text_from_delimited;

pub(super) fn readable_text_for_document(
    bytes: &[u8],
    path: &Path,
    content_type: Option<&str>,
) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let type_hint = content_type.unwrap_or("").to_ascii_lowercase();
    if ext == "pdf" || type_hint.contains("pdf") {
        return Ok(readable_text_from_pdf(bytes));
    }
    let raw = String::from_utf8_lossy(bytes).to_string();
    if ext == "ipynb" {
        return readable_text_from_notebook(&raw);
    }
    if ext == "json" || type_hint.contains("json") {
        return readable_text_from_json(&raw);
    }
    if ext == "csv" || type_hint.contains("csv") {
        return Ok(readable_text_from_delimited(&raw, ','));
    }
    if ext == "tsv"
        || ext == "tab"
        || type_hint.contains("tsv")
        || type_hint.contains("tab-separated-values")
    {
        return Ok(readable_text_from_delimited(&raw, '\t'));
    }
    if ext == "html" || ext == "htm" || type_hint.contains("html") {
        return Ok(readable_text_from_html(&raw));
    }
    if ext == "xml"
        || ext == "rss"
        || ext == "atom"
        || type_hint.contains("xml")
        || type_hint.contains("rss")
        || type_hint.contains("atom")
    {
        return Ok(readable_text_from_feed(&raw));
    }
    Ok(raw)
}

pub(crate) fn read_document(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_document requires path")?;
    let max_bytes =
        u64_input(input, "maxBytes", MAX_READ_BYTES).clamp(1_000, MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let readable = readable_text_for_document(&bytes, &file, None)?;
    let selected = if let Some(range) = string_input(input, "range") {
        line_window(&readable, &range)?
    } else {
        (readable.clone(), 0, 0, Vec::new())
    };
    let output_bytes = selected.0.as_bytes();
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    Ok(
        json!({"path": path, "bytes": readable.len(), "truncated": output_bytes.len() > slice.len(), "mimeType": mime_type_for_path(&file), "text": String::from_utf8_lossy(slice), "startLine": if selected.3.is_empty() { Value::Null } else { json!(selected.1) }, "endLine": if selected.3.is_empty() { Value::Null } else { json!(selected.2) }, "ranges": if selected.3.is_empty() { Value::Null } else { json!(selected.3) }, "sha256": sha256_hex(&bytes)}),
    )
}

pub(crate) fn fetch_readable_url(
    _runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<Value, String> {
    let url = string_input(input, "url").ok_or("fetch_readable_url requires url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("fetch_readable_url requires http(s) URL".into());
    }
    let max_bytes = u64_input(input, "maxBytes", 200_000).clamp(1_000, 1_000_000) as usize;
    let client = crate::net::blocking_builder()
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let ok = (200..300).contains(&status);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let url_path = url::Url::parse(&url)
        .ok()
        .map(|url| url.path().to_string())
        .unwrap_or_default();
    let readable =
        readable_text_for_document(&bytes, Path::new(&url_path), content_type.as_deref())?;
    let selected = if let Some(range) = string_input(input, "range") {
        line_window(&readable, &range)?
    } else {
        (readable.clone(), 0, 0, Vec::new())
    };
    let output_bytes = selected.0.as_bytes();
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    Ok(
        json!({"url": url, "status": status, "ok": ok, "contentType": content_type, "bytes": readable.len(), "sha256": sha256_hex(&bytes), "truncated": output_bytes.len() > slice.len(), "text": String::from_utf8_lossy(slice), "startLine": if selected.3.is_empty() { Value::Null } else { json!(selected.1) }, "endLine": if selected.3.is_empty() { Value::Null } else { json!(selected.2) }, "ranges": if selected.3.is_empty() { Value::Null } else { json!(selected.3) }}),
    )
}
