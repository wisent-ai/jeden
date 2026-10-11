//! The view a turn is given: the chat's oldest messages as a row of tree
//! nodes (the frontier), covering every message before the tail, and the
//! newest messages word for word (the tail). Together they never take more
//! bytes than the operator's view size, however long the chat grows.
//!
//! A view changes only when it outgrows its size, and then in one batch, so
//! the beginning a model provider has cached stays the same for as many
//! turns as possible: first every tail message but the newest folds into the
//! frontier as its leaf (appended at the frontier's end, the beginning
//! untouched); then, while still too large, every adjacent pair of nodes of
//! the finest span merges into its parent at once. The view is kept in
//! `view.json`, so a resumed session continues from it.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::log::Entry;
use super::tree::{Node, Summarize, Tree};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// The nodes covering every message before the tail, oldest first.
    pub frontier: Vec<String>,
    /// The first message given word for word.
    pub tail_start: u64,
    /// How many bytes the view took when it was last fitted.
    pub bytes: usize,
    /// How many messages the log held then.
    pub log_len: u64,
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join("view.json")
}

impl View {
    /// The kept view, or the empty one a new chat starts from. A view that
    /// cannot be read is refused naming the file, never restarted.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = path(dir);
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                format!(
                    "the chat memory view {} is not a view ({error})",
                    path.display()
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!(
                "the chat memory view {} cannot be read: {error}",
                path.display()
            )),
        }
    }

    /// Keep the view, replaced whole through a file beside it.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = path(dir);
        let staged = dir.join("view.json.staged");
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("the chat memory view cannot be encoded: {error}"))?;
        fs::write(&staged, text)
            .and_then(|()| fs::rename(&staged, &path))
            .map_err(|error| {
                format!(
                    "the chat memory view {} cannot be written: {error}",
                    path.display()
                )
            })
    }

    /// The frontier's nodes, read from the tree.
    pub fn nodes(&self, tree: &Tree) -> Result<Vec<Node>, String> {
        self.frontier
            .iter()
            .map(|id| {
                tree.get(id).cloned().ok_or_else(|| {
                    format!("the chat memory view names node {id}, which its summary tree does not hold")
                })
            })
            .collect()
    }

    /// The messages given word for word. A tail that starts past the log is
    /// a view of another chat, and is refused.
    pub fn tail<'a>(&self, entries: &'a [Entry]) -> Result<&'a [Entry], String> {
        entries.get(self.tail_start as usize..).ok_or_else(|| {
            format!(
                "the chat memory view starts its tail at message {}, but the chat log holds {} message(s)",
                self.tail_start,
                entries.len()
            )
        })
    }

    /// Fit the view to `view_bytes` over `entries`, making the leaves and
    /// parents it needs. Answers its size. A view whose newest message alone,
    /// or whose frontier with no two adjacent nodes of equal span left to
    /// merge, still outgrows the size is refused with what it holds.
    pub fn fit(
        &mut self,
        entries: &[Entry],
        tree: &mut Tree,
        view_bytes: usize,
        summary_bytes: usize,
        summarize: &Summarize,
    ) -> Result<usize, String> {
        let mut nodes = self.nodes(tree)?;
        loop {
            let tail = self.tail(entries)?;
            let tail_bytes = tail.iter().map(Entry::bytes).sum::<usize>();
            let size = nodes
                .iter()
                .map(|node| node.id.len() + node.summary.len())
                .sum::<usize>()
                + tail_bytes;
            if size <= view_bytes {
                self.bytes = size;
                self.log_len = entries.len() as u64;
                return Ok(size);
            }
            // Every tail message before the newest turn folds at once, so the
            // view has room again for as many turns as it can hold before it
            // changes next. The tail never starts at a tool result: a
            // provider refuses a tool result whose call is not just before
            // it, so the newest assistant turn that called tools stays with
            // its results.
            let keep_from = tail.iter().rposition(|entry| entry.role != "tool");
            if let Some(older) = keep_from
                .map(|start| &tail[..start])
                .filter(|older| !older.is_empty())
            {
                for entry in older {
                    let leaf = tree.leaf(entry, summary_bytes, summarize)?;
                    self.tail_start = leaf.last + std::num::NonZeroU64::MIN.get();
                    self.frontier.push(leaf.id.clone());
                    nodes.push(leaf);
                }
                continue;
            }
            let merged = merge_finest(&nodes, tree, summary_bytes, summarize)?.ok_or_else(|| {
                format!(
                    "the chat memory view takes {size} bytes, more than chatMemory.viewBytes {view_bytes}: \
                     {} node(s) with no two adjacent of equal span left to merge, and the newest turn \
                     ({} message(s): the newest one, or an assistant turn with its tool results) alone takes {tail_bytes} bytes",
                    nodes.len(),
                    tail.len()
                )
            })?;
            self.frontier = merged.iter().map(|node| node.id.clone()).collect();
            nodes = merged;
        }
    }
}

/// The frontier with every adjacent pair of nodes of the finest span that
/// has one merged into its parent, left to right, in one batch; `None` when
/// no two adjacent nodes have equal span.
fn merge_finest(
    nodes: &[Node],
    tree: &mut Tree,
    summary_bytes: usize,
    summarize: &Summarize,
) -> Result<Option<Vec<Node>>, String> {
    let mut finest: Option<u64> = None;
    let mut walk = nodes.iter().peekable();
    while let Some(node) = walk.next() {
        if let Some(next) = walk.peek() {
            if next.span() == node.span() && finest.is_none_or(|span| node.span() < span) {
                finest = Some(node.span());
            }
        }
    }
    let Some(span) = finest else {
        return Ok(None);
    };
    let mut merged = Vec::with_capacity(nodes.len());
    let mut walk = nodes.iter().peekable();
    while let Some(node) = walk.next() {
        match walk.peek() {
            Some(next) if node.span() == span && next.span() == span => {
                merged.push(tree.parent(node, next, summary_bytes, summarize)?);
                walk.next();
            }
            _ => merged.push(node.clone()),
        }
    }
    Ok(Some(merged))
}
