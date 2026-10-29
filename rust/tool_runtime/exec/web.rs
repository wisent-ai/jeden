//! Fetching one document over the network for a turn to read.
//!
//! Split out of `tool_runtime/exec/mod.rs`, which had grown past the module
//! line cap.

use super::super::shared::{count_input, selected_lines, string_input};
use super::super::ToolRuntime;
use crate::tally::STEP;
use crate::tool_runtime::runtime_ops::context::OperationProgress;
use crate::tool_runtime::runtime_ops::output::BoundedOutput;
use crate::tool_runtime::runtime_ops::output::OutputLimits;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

pub(crate) fn fetch_url(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let url = string_input(input, "url").ok_or("fetch_url requires url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("fetch_url requires http(s) URL".into());
    }
    let max_bytes = count_input(input, "maxBytes", "fetch_url")? as usize;
    if runtime.operation.cancellation().is_cancelled() {
        return Err("fetch_url cancelled".into());
    }
    // The server answers or the connection ends; a cancelled turn still stops
    // this read at the next chunk.
    let client = crate::net::blocking_builder().build().map_err(|error| {
        format!(
            "fetch_url: the HTTP client cannot be built: {}",
            crate::control_plane::transport::describe_reqwest(error)
        )
    })?;
    let response = stado_wait::request_blocking(client.get(&url))
        .map_err(|error| format!("fetch_url {url}: {error}"))?;
    let status = response.status();
    // A header that is not visible text is answered as its bytes read lossily.
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    // The answer keeps the first half of `maxBytes` (one bit shifted off)
    // and the last bytes for the rest; the whole body goes to the artifact.
    let head_bytes = max_bytes >> STEP;
    let mut capture = BoundedOutput::new(
        "fetch",
        Some(OutputLimits {
            head_bytes,
            tail_bytes: max_bytes - head_bytes,
        }),
        runtime.operation.artifacts().clone(),
    );
    let mut body = BufReader::new(response);
    let mut received: Option<u64> = None;
    loop {
        if runtime.operation.cancellation().is_cancelled() {
            return Err("fetch_url cancelled".into());
        }
        let chunk = body
            .fill_buf()
            .map_err(|error| format!("fetch_url {url}: the body could not be read: {error}"))?;
        if chunk.is_empty() {
            break;
        }
        let count = chunk.len();
        capture
            .write_chunk(chunk)
            .map_err(|e| format!("failed capturing fetch response: {e}"))?;
        body.consume(count);
        let total = match received {
            Some(before) => before.saturating_add(count as u64),
            None => count as u64,
        };
        received = Some(total);
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
        return Err(match &artifact {
            Some(artifact) => format!(
                "fetch_url cannot apply a line range beyond maxBytes; full response saved at {artifact}"
            ),
            None => "fetch_url cannot apply a line range beyond maxBytes, and the full response was not kept".to_owned(),
        });
    }
    let window = selected_lines(&captured.text, string_input(input, "range"))?;
    let text = match &window {
        Some(window) => window.text.clone(),
        None => captured.text.clone(),
    };
    Ok(json!({
        "ok": status.is_success(),
        "url": url,
        "status": status.as_u16(),
        "contentType": content_type,
        "bytes": captured.total_bytes,
        "truncated": captured.truncated,
        "sha256": captured.sha256,
        "text": text,
        "head": captured.head,
        "tail": captured.tail,
        "artifact": artifact,
        "startLine": window.as_ref().map(|window| window.start_line),
        "endLine": window.as_ref().map(|window| window.end_line),
        "ranges": window.as_ref().map(|window| &window.ranges),
    }))
}
