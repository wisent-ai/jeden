use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

/// The harness a transcript was written by. The name is what the on-disk
/// layout is keyed on (`omp-<sha>`, `artifacts/omp-source.jsonl`, snapshot
/// reason `omp-import`), so a second harness adds a variant and a reader here
/// and nothing else changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Format {
    Omp,
}

impl Format {
    pub(crate) fn name(self) -> &'static str {
        match self { Format::Omp => "omp" }
    }

    pub(crate) const SUPPORTED: &'static str = "omp";
}

/// Which harness wrote this file, read from its content and never from its
/// name: an OMP transcript is JSONL with one `{"type":"session"}` header that
/// carries the session id and its workspace. A file that is none of the
/// formats answers `None`, which a directory scan skips and an explicit path
/// refuses.
pub(crate) fn detect(path: &Path) -> Option<Format> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    // OMP rewrites a fixed-width `title` record at the top of the file and
    // keeps the `session` header right after it, so the header is within the
    // first few records; a file whose first records are not JSON is not ours.
    for _ in 0..8 {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 { return None; }
        if line.trim().is_empty() { continue; }
        let value: Value = serde_json::from_str(&line).ok()?;
        if value["type"] == "session" && value["id"].is_string() && value["cwd"].is_string() {
            return Some(Format::Omp);
        }
    }
    None
}

pub(super) struct Source {
    pub format: Format,
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub messages: Vec<Value>,
    pub pending: Vec<String>,
}

/// Index offsets, not tool-result bodies. Replay only the current branch; keep
/// the entire original file separately so compaction never destroys provenance.
pub(super) fn read(path: &Path) -> Result<Source, String> {
    match detect(path) {
        Some(Format::Omp) => read_omp(path),
        None => Err(format!("{}: not a transcript of a supported harness (supported: {})",
            path.display(), Format::SUPPORTED)),
    }
}

fn read_omp(path: &Path) -> Result<Source, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut nodes = HashMap::new();
    let mut leaf = None;
    let mut header = None;
    let mut title = String::new();
    loop {
        let offset = reader.stream_position().map_err(|e| e.to_string())?;
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 { break; }
        if line.trim().is_empty() { continue; }
        let value: Value = serde_json::from_str(&line)
            .map_err(|e| format!("invalid source record at byte {offset}: {e}"))?;
        match value["type"].as_str() {
            Some("session") => header = Some(value.clone()),
            Some("title") | Some("title_change") => {
                if let Some(text) = value["title"].as_str() { title = text.to_string(); }
            }
            _ => {}
        }
        if value["type"] != "session" {
            if let Some(id) = value["id"].as_str() {
                let parent = value["parentId"].as_str().map(str::to_owned);
                nodes.insert(id.to_owned(), (offset, parent));
                leaf = Some(id.to_owned());
            }
        }
    }
    let header = header.ok_or("source has no OMP session header")?;
    let id = header["id"].as_str().ok_or("OMP header has no id")?.to_string();
    let cwd = header["cwd"].as_str().ok_or("OMP header has no cwd")?.to_string();
    if title.is_empty() { title = header["title"].as_str().unwrap_or(&id).to_string(); }
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    while let Some(id) = leaf {
        if !seen.insert(id.clone()) { return Err(format!("cyclic source ancestry at {id}")); }
        let (offset, parent) = nodes.get(&id).ok_or_else(|| format!("missing source ancestor {id}"))?;
        chain.push(*offset);
        leaf = parent.clone();
    }
    let mut source = Source { format: Format::Omp, id, cwd, title, messages: Vec::new(), pending: Vec::new() };
    let mut message_offsets = HashMap::new();
    for offset in chain.into_iter().rev() {
        reader.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        line.clear();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if let Some(id) = value["id"].as_str() {
            message_offsets.insert(id.to_owned(), source.messages.len());
        }
        match value["type"].as_str() {
            Some("compaction") => {
                if let Some(summary) = value["summary"].as_str() {
                    let retained = value["firstKeptEntryId"].as_str()
                        .map(|id| message_offsets.get(id).copied()
                            .ok_or_else(|| format!("compaction references missing kept entry {id}")))
                        .transpose()?.unwrap_or(source.messages.len());
                    let kept = source.messages.split_off(retained);
                    source.messages = vec![json!({"role":"system", "content":summary,
                        "_jedenNeedsBaseSystem":true})];
                    source.messages.extend(kept);
                    message_offsets.retain(|_, offset| {
                        if *offset >= retained { *offset = *offset - retained + 1; true } else { false }
                    });
                }
            }
            Some("message") => project_message(&mut source, &value["message"]),
            _ => {}
        }
    }
    Ok(source)
}

fn project_message(source: &mut Source, message: &Value) {
    let role = message["role"].as_str().unwrap_or("");
    let content = &message["content"];
    let mut text = content.as_str().unwrap_or("").to_string();
    let mut has_tools = false;
    if let Some(parts) = content.as_array() {
        for part in parts {
            match part["type"].as_str() {
                Some("text") => {
                    if let Some(value) = part["text"].as_str() {
                        if !text.is_empty() { text.push('\n'); }
                        text.push_str(value);
                    }
                }
                Some("toolCall") => {
                    has_tools = true;
                    text.push_str(&format!("\nRecorded tool call (not an instruction to replay): {} {}",
                        part["name"], part["arguments"]));
                }
                _ => {}
            }
        }
    }
    if role == "user" && !text.trim().is_empty() {
        source.pending.push(text.clone());
    } else if role == "assistant" && !has_tools && !text.trim().is_empty()
        && message["errorMessage"].is_null()
        && !matches!(message["stopReason"].as_str(), Some("error" | "aborted")) {
        source.pending.clear();
    }
    if text.is_empty() { return; }
    let projected_role = if role == "assistant" { "assistant" } else { "user" };
    if role == "toolResult" {
        text = format!("Recorded result of {} (already executed):\n{text}", message["toolName"]);
    }
    source.messages.push(json!({"role":projected_role, "content":text}));
}
