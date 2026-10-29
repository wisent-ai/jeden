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
        match self {
            Format::Omp => "omp",
        }
    }

    pub(crate) const SUPPORTED: &'static str = "omp";
}

/// The record OMP rewrites in place at the top of a transcript; the session
/// header follows it.
const TITLE_RECORD: &str = "title";

/// Which harness wrote this file, read from its content and never from its
/// name: an OMP transcript is JSONL whose first record other than the title
/// OMP keeps at the top is the `{"type":"session"}` header carrying the
/// session id and its workspace. A file that is none of the formats answers
/// `Ok(None)`, which a directory scan skips and an explicit path refuses; a
/// file that cannot be read is an `Err` naming it and the cause, never "not
/// a transcript".
pub(crate) fn detect(path: &Path) -> Result<Option<Format>, String> {
    let unreadable = |e: std::io::Error| format!("{}: cannot be read: {e}", path.display());
    let file = File::open(path).map_err(unreadable)?;
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            // Bytes that are not UTF-8 are not a transcript of ours.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Ok(None),
            Err(e) => return Err(unreadable(e)),
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return Ok(None);
        };
        if value["type"] == TITLE_RECORD {
            continue;
        }
        let header =
            value["type"] == "session" && value["id"].is_string() && value["cwd"].is_string();
        return Ok(header.then_some(Format::Omp));
    }
    Ok(None)
}

pub(super) struct Source {
    pub format: Format,
    pub id: String,
    /// The title the transcript states, or none when it states none.
    pub title: Option<String>,
    pub cwd: String,
    pub messages: Vec<Value>,
    pub pending: Vec<String>,
}

/// Index offsets, not tool-result bodies. Replay only the current branch; keep
/// the entire original file separately so compaction never destroys provenance.
pub(super) fn read(path: &Path) -> Result<Source, String> {
    match detect(path)? {
        Some(Format::Omp) => read_omp(path),
        None => Err(format!(
            "{}: not a transcript of a supported harness (supported: {})",
            path.display(),
            Format::SUPPORTED
        )),
    }
}

fn read_omp(path: &Path) -> Result<Source, String> {
    let at = |e: std::io::Error| format!("{}: {e}", path.display());
    let file = File::open(path).map_err(at)?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut nodes = HashMap::new();
    let mut leaf = None;
    let mut header = None;
    let mut title = None;
    loop {
        let offset = reader.stream_position().map_err(at)?;
        line.clear();
        reader.read_line(&mut line).map_err(at)?;
        // A read that adds nothing is the end of the file; a blank line
        // still carries its line break.
        if line.is_empty() {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(|e| {
            format!(
                "{}: invalid source record at byte {offset}: {e}",
                path.display()
            )
        })?;
        match value["type"].as_str() {
            Some("session") => header = Some(value.clone()),
            Some(TITLE_RECORD) | Some("title_change") => {
                if let Some(text) = value["title"].as_str() {
                    title = Some(text.to_string());
                }
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
    let id = header["id"]
        .as_str()
        .ok_or("OMP header has no id")?
        .to_string();
    let cwd = header["cwd"]
        .as_str()
        .ok_or("OMP header has no cwd")?
        .to_string();
    // A title record wins over the header's own; a transcript with neither
    // has no title, and the import says so rather than calling it by its id.
    let title = title.or_else(|| header["title"].as_str().map(str::to_string));
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    while let Some(id) = leaf {
        if !seen.insert(id.clone()) {
            return Err(format!("cyclic source ancestry at {id}"));
        }
        let (offset, parent) = nodes
            .get(&id)
            .ok_or_else(|| format!("missing source ancestor {id}"))?;
        chain.push(*offset);
        leaf = parent.clone();
    }
    let mut source = Source {
        format: Format::Omp,
        id,
        cwd,
        title,
        messages: Vec::new(),
        pending: Vec::new(),
    };
    let mut message_offsets = HashMap::new();
    for offset in chain.into_iter().rev() {
        reader.seek(SeekFrom::Start(offset)).map_err(at)?;
        line.clear();
        reader.read_line(&mut line).map_err(at)?;
        let value: Value = serde_json::from_str(&line).map_err(|e| {
            format!(
                "{}: invalid source record at byte {offset}: {e}",
                path.display()
            )
        })?;
        if let Some(id) = value["id"].as_str() {
            message_offsets.insert(id.to_owned(), source.messages.len());
        }
        match value["type"].as_str() {
            Some("compaction") => {
                if let Some(summary) = value["summary"].as_str() {
                    // The kept tail starts at the entry the compaction names;
                    // one that names none summarises everything before it.
                    let retained = match value["firstKeptEntryId"].as_str() {
                        Some(kept) => message_offsets.get(kept).copied().ok_or_else(|| {
                            format!("compaction references missing kept entry {kept}")
                        })?,
                        None => source.messages.len(),
                    };
                    let kept = source.messages.split_off(retained);
                    source.messages = vec![json!({"role":"system", "content":summary,
                        "_jedenNeedsBaseSystem":true})];
                    let shift = source.messages.len();
                    source.messages.extend(kept);
                    message_offsets.retain(|_, offset| {
                        if *offset >= retained {
                            *offset = *offset - retained + shift;
                            true
                        } else {
                            false
                        }
                    });
                }
            }
            Some("message") => project_message(&mut source, &value["message"])
                .map_err(|e| format!("{}: record at byte {offset}: {e}", path.display()))?,
            _ => {}
        }
    }
    Ok(source)
}

/// The text a message carries: its content when that is text, or its text
/// parts and recorded tool calls joined, refused when it is neither.
fn message_text(content: &Value) -> Result<(String, bool), String> {
    if let Some(text) = content.as_str() {
        return Ok((text.to_string(), false));
    }
    let parts = content
        .as_array()
        .ok_or_else(|| format!("message content is neither text nor a list of parts: {content}"))?;
    let mut text = String::new();
    let mut has_tools = false;
    for part in parts {
        match part["type"].as_str() {
            Some("text") => {
                let value = part["text"]
                    .as_str()
                    .ok_or_else(|| format!("a text part carries no text: {part}"))?;
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(value);
            }
            Some("toolCall") => {
                has_tools = true;
                text.push_str(&format!(
                    "\nRecorded tool call (not an instruction to replay): {} {}",
                    part["name"], part["arguments"]
                ));
            }
            _ => {}
        }
    }
    Ok((text, has_tools))
}

fn project_message(source: &mut Source, message: &Value) -> Result<(), String> {
    let role = message["role"]
        .as_str()
        .ok_or_else(|| format!("message has no role: {message}"))?;
    let (mut text, has_tools) = message_text(&message["content"])?;
    if role == "user" && !text.trim().is_empty() {
        source.pending.push(text.clone());
    } else if role == "assistant"
        && !has_tools
        && !text.trim().is_empty()
        && message["errorMessage"].is_null()
        && !matches!(message["stopReason"].as_str(), Some("error" | "aborted"))
    {
        source.pending.clear();
    }
    if text.is_empty() {
        return Ok(());
    }
    let projected_role = if role == "assistant" {
        "assistant"
    } else {
        "user"
    };
    if role == "toolResult" {
        text = format!(
            "Recorded result of {} (already executed):\n{text}",
            message["toolName"]
        );
    }
    source
        .messages
        .push(json!({"role":projected_role, "content":text}));
    Ok(())
}
