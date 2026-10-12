//! Reading a document off disk or off the network as text a model can use.
//!
//! Which reader turns the bytes into text is decided by the bytes: a PDF's
//! file header, a JSON document that parses (a Jupyter notebook is the JSON
//! whose top level carries `nbformat` and a `cells` array, as the nbformat
//! specification requires), markup that opens with `<` (a feed's items, or
//! the page's text when it has none), or rows with the same number of tab or
//! comma separators on every line. A file's extension or a server's
//! content-type label is not asked: a list of spellings read `notes.txt`
//! holding JSON as plain text and missed every format it did not name.

use serde_json::{json, Value};
use std::fs;

use super::PDF_TYPE;
use crate::tool_runtime::shared::{
    count_input, jail_path, line_window, mime_type_of, sha256_hex, string_input,
};
use crate::tool_runtime::ToolRuntime;

mod formats;
mod tables;

use formats::{
    readable_text_from_feed, readable_text_from_html, readable_text_from_json,
    readable_text_from_notebook, readable_text_from_pdf,
};
use tables::readable_text_from_delimited;

/// The separators a delimited table may use, tried in this order.
const TAB: char = '\t';
const COMMA: char = ',';
/// The top-level keys every Jupyter notebook carries (nbformat v4 schema).
const NOTEBOOK_FORMAT_KEY: &str = "nbformat";
const NOTEBOOK_CELLS_KEY: &str = "cells";

/// Whether every non-empty line of `raw` holds `delimiter` the same number of
/// times, and the first holds it at all: the structure of a delimited table.
fn delimited_by(raw: &str, delimiter: char) -> bool {
    let mut lines = raw.lines().filter(|line| !line.trim().is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    let columns = first.matches(delimiter).count();
    first.contains(delimiter) && lines.all(|line| line.matches(delimiter).count() == columns)
}

pub(super) fn readable_text_for_document(bytes: &[u8]) -> Result<String, String> {
    if mime_type_of(bytes) == PDF_TYPE {
        return Ok(readable_text_from_pdf(bytes));
    }
    let raw = String::from_utf8_lossy(bytes).to_string();
    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
        let notebook = value.get(NOTEBOOK_FORMAT_KEY).is_some()
            && value.get(NOTEBOOK_CELLS_KEY).is_some_and(Value::is_array);
        return if notebook {
            readable_text_from_notebook(&raw)
        } else {
            readable_text_from_json(&raw)
        };
    }
    if raw.trim_start().starts_with('<') {
        return Ok(readable_text_from_feed(&raw));
    }
    if delimited_by(&raw, TAB) {
        return Ok(readable_text_from_delimited(&raw, TAB));
    }
    if delimited_by(&raw, COMMA) {
        return Ok(readable_text_from_delimited(&raw, COMMA));
    }
    Ok(raw)
}

pub(crate) fn read_document(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_document requires path")?;
    let max_bytes = count_input(input, "maxBytes", "read_document")? as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let readable = readable_text_for_document(&bytes)?;
    let selected = if let Some(range) = string_input(input, "range") {
        line_window(&readable, &range)?
    } else {
        (readable.clone(), 0, 0, Vec::new())
    };
    let output_bytes = selected.0.as_bytes();
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    Ok(
        json!({"path": path, "bytes": readable.len(), "truncated": output_bytes.len() > slice.len(), "mimeType": mime_type_of(&bytes), "text": String::from_utf8_lossy(slice), "startLine": if selected.3.is_empty() { Value::Null } else { json!(selected.1) }, "endLine": if selected.3.is_empty() { Value::Null } else { json!(selected.2) }, "ranges": if selected.3.is_empty() { Value::Null } else { json!(selected.3) }, "sha256": sha256_hex(&bytes)}),
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
    let max_bytes = count_input(input, "maxBytes", "fetch_readable_url")? as usize;
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
    let readable = readable_text_for_document(&bytes)?;
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
