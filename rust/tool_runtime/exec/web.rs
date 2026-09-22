//! Fetching one document over the network for a turn to read.
//!
//! Split out of `tool_runtime/exec/mod.rs`, which had grown past the module
//! line cap.

use super::super::shared::{line_window, string_input, u64_input};
use super::super::ToolRuntime;
use serde_json::{json, Value};
use std::io::Read;

pub(crate) fn fetch_url(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let url = string_input(input, "url").ok_or("fetch_url requires url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("fetch_url requires http(s) URL".into());
    }
    let max_bytes = u64_input(input, "maxBytes", 200_000).clamp(1_000, 1_000_000) as usize;
    if runtime.operation.cancellation().is_cancelled() {
        return Err("fetch_url cancelled".into());
    }
    // The server answers or the connection ends; a cancelled turn still stops
    // this read at the next chunk.
    let client = crate::net::blocking_builder()
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let mut capture = BoundedOutput::new(
        "fetch",
        OutputLimits {
            head_bytes: max_bytes / 2,
            tail_bytes: max_bytes - (max_bytes / 2),
        },
        runtime.operation.artifacts().clone(),
    );
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        if runtime.operation.cancellation().is_cancelled() {
            return Err("fetch_url cancelled".into());
        }
        let count = response.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        capture
            .write_chunk(&buffer[..count])
            .map_err(|e| format!("failed capturing fetch response: {e}"))?;
        total = total.saturating_add(count as u64);
        runtime.operation.progress(OperationProgress {
            stream: "fetch",
            bytes: count as u64,
            total_bytes: total,
        });
    }
    let captured = capture.finish().map_err(|e| e.to_string())?;
    let artifact = captured
        .artifact
        .as_ref()
        .map(|path| path.display().to_string());
    if captured.truncated && input.get("range").is_some() {
        return Err(format!(
            "fetch_url cannot apply a line range beyond maxBytes; full response saved at {}",
            artifact.as_deref().unwrap_or("artifact sink")
        ));
    }
    let (text, start_line, end_line, ranges) = if let Some(range) = string_input(input, "range") {
        line_window(&captured.text, &range)?
    } else {
        (captured.text, 0, 0, Vec::new())
    };
    Ok(json!({
        "ok": (200..300).contains(&status),
        "url": url,
        "status": status,
        "contentType": content_type,
        "bytes": captured.total_bytes,
        "truncated": captured.truncated,
        "sha256": captured.sha256,
        "text": text,
        "head": captured.head,
        "tail": captured.tail,
        "artifact": artifact,
        "startLine": if ranges.is_empty() { Value::Null } else { json!(start_line) },
        "endLine": if ranges.is_empty() { Value::Null } else { json!(end_line) },
        "ranges": if ranges.is_empty() { Value::Null } else { json!(ranges) }
    }))
}
