//! Reading a chat memory back: zoom into a node, the messages of a time, and
//! the whole memory as the CLI and Jeden Desktop show it. Nothing here
//! writes.

use serde_json::{json, Value};
use std::path::Path;

use super::super::{summary_bytes, Entry, Log, Tree, View};

fn entry_json(entry: &Entry) -> Value {
    let mut shown = entry.message();
    shown["index"] = json!(entry.index);
    shown["at"] = json!(entry.at);
    shown
}

/// A node's two halves as nodes, or, for a leaf, its message word for word.
/// A node the tree does not hold is refused naming it.
pub fn zoom(dir: &Path, id: &str) -> Result<Value, String> {
    let tree = Tree::open(dir)?;
    let node = tree.get(id).ok_or_else(|| {
        format!("the chat memory holds no node {id}; its view names the nodes it has")
    })?;
    if node.children.is_empty() {
        let entries = Log::at(dir).read()?;
        let entry = entries
            .iter()
            .find(|entry| entry.index == node.first)
            .ok_or_else(|| {
                format!(
                    "node {id} covers message {}, which the chat log {} does not hold",
                    node.first,
                    Log::at(dir).path().display()
                )
            })?;
        return Ok(json!({ "node": node, "message": entry_json(entry) }));
    }
    let children: Vec<Value> = node
        .children
        .iter()
        .map(|child| {
            tree.get(child).map(|child| json!(child)).ok_or_else(|| {
                format!("node {id} names its half {child}, which the summary tree does not hold")
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(json!({ "node": node, "children": children }))
}

fn seconds(text: &str, what: &str) -> Result<u64, String> {
    text.trim().parse().map_err(|error| {
        format!("{what} is a time in seconds since 1970; `{text}` is not ({error})")
    })
}

/// Every message that entered the log between `from` and `to`, inclusive,
/// in seconds since 1970, word for word.
pub fn dates(dir: &Path, from: &str, to: &str) -> Result<Value, String> {
    let (from, to) = (seconds(from, "from")?, seconds(to, "to")?);
    if from > to {
        return Err(format!(
            "from {from} is after to {to}; no message entered the log in that time"
        ));
    }
    let mut messages = Vec::new();
    for entry in Log::at(dir).read()? {
        let at = seconds(&entry.at, &format!("message {}'s time", entry.index))?;
        if (from..=to).contains(&at) {
            messages.push(entry_json(&entry));
        }
    }
    Ok(json!({ "from": from, "to": to, "messages": messages }))
}

/// The memory of a session as a whole: how long the log is, the view a turn
/// is given now, and every node of the tree.
pub fn describe(dir: &Path) -> Result<Value, String> {
    let log = Log::at(dir);
    let entries = log.read()?;
    let tree = Tree::open(dir)?;
    let view = View::load(dir)?;
    let frontier = view.nodes(&tree)?;
    // In the order of the chat, not of their ids as text ("10-10" before "2-2").
    let mut ordered: Vec<_> = tree.nodes().collect();
    ordered.sort_by_key(|node| (node.first, node.last));
    let nodes: Vec<Value> = ordered.into_iter().map(|node| json!(node)).collect();
    Ok(json!({
        "directory": dir,
        "log": { "path": log.path(), "messages": entries.len() },
        "summaryBytes": summary_bytes(),
        "view": {
            "frontier": frontier,
            "tailStart": view.tail_start,
            "tail": view.tail(&entries)?.iter().map(entry_json).collect::<Vec<_>>(),
            "bytes": view.bytes,
            "logLength": view.log_len,
        },
        "nodes": nodes,
    }))
}
