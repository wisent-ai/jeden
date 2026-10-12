//! The view a turn is given: a list of tree nodes that covers the whole chat
//! before the turn, oldest first, one `id+n|text` line each inside `<chat>`
//! tags. It holds only summaries, never a whole message.
//!
//! Each new message appends its line and nothing else changes. Once the view
//! passes the high size, one batch merges the most due sibling pairs until
//! it is at most the low size, so the view is a sawtooth between the two and
//! its beginning, which the provider caches, stays put between batches. For
//! siblings (l, i) and (l, i+1), with T messages in the chat,
//!
//!   due = (T - last) / 2^l      last: the pair's last message
//!
//! and among equally due pairs the oldest merges first. The view is saved to
//! `view.json` as its `[l, i]` pairs and loaded at start; it is never
//! rebuilt from the log, since a rebuilt view differs from the live one.

use serde::{Deserialize, Serialize};
use std::fs;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};

use super::log::Entry;
use super::tree::{count, halves, last, Node, Summarize, Tree};

/// How a view's lines are built and merged: the operator's numbers and the
/// cheap model.
pub struct Fitting<'a> {
    pub summary_bytes: usize,
    pub attempts: usize,
    pub high: usize,
    pub low: usize,
    pub summarize: &'a Summarize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct View(pub Vec<(u32, u64)>);

pub fn path(dir: &Path) -> PathBuf {
    dir.join("view.json")
}

/// The lines `nodes` give inside `<chat>` tags.
pub fn render(nodes: &[Node]) -> String {
    let mut text = String::from("<chat>\n");
    for node in nodes {
        text.push_str(&node.line());
    }
    text.push_str("</chat>\n");
    text
}

/// How many bytes `nodes` take as lines.
pub fn bytes(nodes: &[Node]) -> usize {
    nodes.iter().map(|node| node.line().len()).sum()
}

impl View {
    /// The saved view, or the empty one a new chat starts from. A view that
    /// cannot be read is refused naming the file, never rebuilt.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = path(dir);
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                format!(
                    "the chat memory view {} is not a list of [l, i] pairs ({error})",
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

    /// Save the view, replaced whole through a file beside it.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = path(dir);
        let staged = dir.join("view.json.staged");
        let text = serde_json::to_string(self)
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

    /// How many messages the view covers: its nodes cover the chat from its
    /// first message on, without a gap.
    pub fn covered(&self) -> u64 {
        self.0.iter().map(|(l, _)| count(*l)).sum()
    }

    /// The view's nodes, read from the tree.
    pub fn nodes(&self, tree: &Tree) -> Result<Vec<Node>, String> {
        self.0
            .iter()
            .map(|(l, i)| {
                tree.get(*l, *i).cloned().ok_or_else(|| {
                    format!(
                        "the chat memory view names node {}, which its summary tree does not hold",
                        super::tree::name(*l, *i)
                    )
                })
            })
            .collect()
    }

    /// Append the line of every message in `entries` the view does not
    /// cover yet, building each from the lines before it, then, once the
    /// view passes the high size, merge it down to the low size in one
    /// batch. Answers whether a batch merged.
    pub fn grow(
        &mut self,
        entries: &[Entry],
        tree: &mut Tree,
        fitting: &Fitting,
    ) -> Result<bool, String> {
        let mut nodes = self.nodes(tree)?;
        let covered = usize::try_from(self.covered()).map_err(|error| {
            format!("the view covers more messages than this machine counts: {error}")
        })?;
        let fresh = entries.get(covered..).ok_or_else(|| {
            format!(
                "the chat memory view covers {covered} message(s), but the chat log holds {}",
                entries.len()
            )
        })?;
        for entry in fresh {
            let leaf = tree.leaf(
                entry,
                &render(&nodes),
                fitting.summary_bytes,
                fitting.attempts,
                fitting.summarize,
            )?;
            self.0.push((leaf.l, leaf.i));
            nodes.push(leaf);
        }
        if bytes(&nodes) <= fitting.high {
            return Ok(false);
        }
        let total = self.covered();
        while bytes(&nodes) > fitting.low {
            let Some(at) = most_due(&nodes, total) else {
                break;
            };
            let (l, i) = parent_of(&nodes[at]);
            let parent = tree.parent(
                l,
                i,
                &render(&nodes[..=at + NonZeroUsize::MIN.get()]),
                fitting.summary_bytes,
                fitting.attempts,
                fitting.summarize,
            )?;
            nodes.splice(at..=at + NonZeroUsize::MIN.get(), [parent]);
        }
        self.0 = nodes.iter().map(|node| (node.l, node.i)).collect();
        Ok(true)
    }
}

/// The parent of the left node of a sibling pair.
fn parent_of(left: &Node) -> (u32, u64) {
    (
        left.l + NonZeroU32::MIN.get(),
        left.i >> NonZeroU32::MIN.get(),
    )
}

/// Where the most due sibling pair of `nodes` starts, the oldest of equally
/// due pairs; none when no two adjacent nodes are siblings.
pub fn most_due(nodes: &[Node], total: u64) -> Option<usize> {
    let mut best: Option<(usize, u128, u32)> = None;
    let pairs = nodes.windows(NonZeroUsize::MIN.get() + NonZeroUsize::MIN.get());
    for (at, window) in pairs.enumerate() {
        let [a, b] = window else {
            continue;
        };
        let (l, i) = parent_of(a);
        if halves(l, i) != Some(((a.l, a.i), (b.l, b.i))) {
            continue;
        }
        let since = u128::from(total.saturating_sub(last(l, i)));
        let more_due =
            best.is_none_or(|(_, kept_since, kept_level)| since << kept_level > kept_since << a.l);
        if more_due {
            best = Some((at, since, a.l));
        }
    }
    best.map(|(at, _, _)| at)
}
