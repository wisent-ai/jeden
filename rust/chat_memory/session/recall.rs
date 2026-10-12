//! Reading a chat memory back: zoom(id, n) into a line, date(id) of a
//! message, and the whole memory as the CLI and Jeden Desktop show it.
//! Nothing here writes.

use serde_json::{json, Value};
use std::num::NonZeroU64;
use std::path::Path;

use super::super::tree::{halves, name};
use super::super::view::{bytes, render};
use super::super::{message_level, summary_bytes, view_high, view_low, Entry, Log, Tree, View};

fn number(text: &str, what: &str) -> Result<u64, String> {
    text.trim()
        .parse()
        .map_err(|error| format!("{what} is a whole number of messages; `{text}` is not ({error})"))
}

fn message(dir: &Path, id: u64) -> Result<Entry, String> {
    let log = Log::at(dir);
    let index = usize::try_from(id)
        .map_err(|error| format!("message {id} is beyond what this machine counts: {error}"))?;
    log.read()?
        .get(index)
        .cloned()
        .ok_or_else(|| format!("the chat log {} holds no message {id}", log.dir().display()))
}

/// Line `id+n` opened: for a line of one message, the message whole; else
/// the two lines of n/2 it was made from, a line not built yet shown as
/// such. `n` is a power of two and `id` a multiple of it, as the view names
/// its lines; anything else is refused naming what a line looks like.
pub fn zoom(dir: &Path, id: &str, n: &str) -> Result<Value, String> {
    let (id, n) = (number(id, "id")?, number(n, "n")?);
    if !n.is_power_of_two() || id / n * n != id {
        return Err(format!(
            "{id}+{n} is no line of the view: a line covers a power of two messages from a multiple of that count, as `id+n|` names it"
        ));
    }
    if n == NonZeroU64::MIN.get() {
        let entry = message(dir, id)?;
        return Ok(json!({ "line": name(message_level(), id), "message": entry }));
    }
    let level = n.trailing_zeros();
    let (left, right) = halves(level, id >> level)
        .ok_or_else(|| format!("{id}+{n} is a message's own line, made of no halves"))?;
    let tree = Tree::open(dir)?;
    let line = |(l, i): (u32, u64)| match tree.get(l, i) {
        Some(node) => node.line(),
        None => format!("{}|(not summarized yet: zoom it)\n", name(l, i)),
    };
    Ok(json!({
        "line": name(level, id >> level),
        "lines": [line(left), line(right)],
    }))
}

/// When message `id` entered the log.
pub fn date(dir: &Path, id: &str) -> Result<Value, String> {
    let id = number(id, "id")?;
    let entry = message(dir, id)?;
    Ok(json!({ "id": id, "date": entry.date }))
}

/// The memory of a session as a whole: the log, the view a turn is given
/// now and its size between the design's bounds, and every node built.
pub fn describe(dir: &Path) -> Result<Value, String> {
    let log = Log::at(dir);
    let entries = log.read()?;
    let tree = Tree::open(dir)?;
    let view = View::load(dir)?;
    let nodes = view.nodes(&tree)?;
    Ok(json!({
        "directory": dir,
        "log": { "directory": log.dir(), "messages": entries.len() },
        "summaryBytes": summary_bytes(),
        "view": {
            "pairs": view.0,
            "covers": view.covered(),
            "bytes": bytes(&nodes),
            "high": view_high(),
            "low": view_low(),
            "text": render(&nodes),
        },
        "nodes": tree.nodes().collect::<Vec<_>>(),
    }))
}
