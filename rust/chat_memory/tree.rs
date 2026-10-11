//! The summary tree over the chat log: a leaf per message, a parent per two
//! adjacent nodes of equal span, each holding a summary no longer than the
//! operator's summary size. Because only equal spans are paired, a node's
//! span is its height: a leaf covers one message, its parent two, theirs
//! four. Nodes are appended to `tree.jsonl` and never rewritten, so a node,
//! once made, says the same thing in every later view.
//!
//! A message that already fits the summary size is its own leaf, word for
//! word, and so is a pair whose two summaries fit together: nothing is asked
//! of a model that the text itself already answers. Anything longer is
//! summarized by the cheap model the operator named, through Brama.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use super::log::Entry;

/// One node of the tree.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// `<first>-<last>`: the messages it covers, which name it.
    pub id: String,
    /// The first and last message it covers, inclusive.
    pub first: u64,
    pub last: u64,
    /// When the first and the last of those messages entered the log.
    pub from: String,
    pub to: String,
    pub summary: String,
    /// `verbatim` when the messages fit word for word, `joined` when two
    /// halves' summaries (one of them a model's) fit side by side, else
    /// `model`.
    pub by: String,
    /// The two nodes it was made of; none for a leaf.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    /// The model's answer was longer than the summary size and was cut at
    /// the last character that fits.
    #[serde(default)]
    pub clipped: bool,
}

impl Node {
    /// How many messages after its first it covers: its height in the tree.
    pub fn span(&self) -> u64 {
        self.last - self.first
    }
}

/// What writes a summary: the cheap model through Brama, given the text and
/// the most bytes the summary may take.
pub type Summarize = dyn Fn(&str, usize) -> Result<String, String> + Send + Sync;

/// The tree file of one session, with every node read from it.
pub struct Tree {
    path: PathBuf,
    nodes: BTreeMap<String, Node>,
}

pub fn node_id(first: u64, last: u64) -> String {
    format!("{first}-{last}")
}

/// `text` cut at the last character boundary within `bytes`, and whether it
/// was cut.
fn within(text: String, bytes: usize) -> (String, bool) {
    if text.len() <= bytes {
        return (text, false);
    }
    let end = text
        .char_indices()
        .map(|(at, character)| at + character.len_utf8())
        .take_while(|end| *end <= bytes)
        .last();
    match end {
        Some(end) => (text[..end].to_string(), true),
        None => (String::new(), true),
    }
}

impl Tree {
    /// The tree of the memory directory `dir`. No file is a tree with no node;
    /// a line that is not a node is refused naming the file.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let path = dir.join("tree.jsonl");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(format!(
                    "the summary tree {} cannot be read: {error}",
                    path.display()
                ))
            }
        };
        let mut nodes = BTreeMap::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let node: Node = serde_json::from_str(line).map_err(|error| {
                format!(
                    "the summary tree {} holds, after its {} node(s), a line that is not a node ({error})",
                    path.display(),
                    nodes.len()
                )
            })?;
            nodes.insert(node.id.clone(), node);
        }
        Ok(Self { path, nodes })
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    fn keep(&mut self, node: Node) -> Result<Node, String> {
        let line = serde_json::to_string(&node)
            .map_err(|error| format!("node {} cannot be encoded: {error}", node.id))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "the chat memory directory {} cannot be created: {error}",
                    parent.display()
                )
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| {
                format!(
                    "the summary tree {} cannot be opened: {error}",
                    self.path.display()
                )
            })?;
        file.write_all(format!("{line}\n").as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                format!(
                    "the summary tree {} cannot be appended to: {error}",
                    self.path.display()
                )
            })?;
        self.nodes.insert(node.id.clone(), node.clone());
        Ok(node)
    }

    /// The leaf of `entry`, made now when the tree has none.
    pub fn leaf(
        &mut self,
        entry: &Entry,
        bytes: usize,
        summarize: &Summarize,
    ) -> Result<Node, String> {
        let id = node_id(entry.index, entry.index);
        if let Some(node) = self.nodes.get(&id) {
            return Ok(node.clone());
        }
        let text = format!("{}: {}", entry.role, entry.text());
        let (summary, by, clipped) = if text.len() <= bytes {
            (text, "verbatim".to_string(), false)
        } else {
            let written = summarize(&text, bytes).map_err(|error| {
                format!("message {} could not be summarized: {error}", entry.index)
            })?;
            let (summary, clipped) = within(written, bytes);
            (summary, "model".to_string(), clipped)
        };
        self.keep(Node {
            id,
            first: entry.index,
            last: entry.index,
            from: entry.at.clone(),
            to: entry.at.clone(),
            summary,
            by,
            children: Vec::new(),
            clipped,
        })
    }

    /// The parent of the adjacent nodes `left` and `right` of equal span, made
    /// now when the tree has none.
    pub fn parent(
        &mut self,
        left: &Node,
        right: &Node,
        bytes: usize,
        summarize: &Summarize,
    ) -> Result<Node, String> {
        let adjacent = left.last.checked_add(NonZeroU64::MIN.get()) == Some(right.first);
        if left.span() != right.span() || !adjacent {
            return Err(format!(
                "nodes {} and {} are not adjacent nodes of equal span, so they have no parent",
                left.id, right.id
            ));
        }
        let id = node_id(left.first, right.last);
        if let Some(node) = self.nodes.get(&id) {
            return Ok(node.clone());
        }
        let joined = format!("{}\n{}", left.summary, right.summary);
        let (summary, by, clipped) = if joined.len() <= bytes {
            // Word for word only when both halves are; a model's summary set
            // beside another stays a summary.
            let by = if left.by == "verbatim" && right.by == left.by {
                left.by.clone()
            } else {
                "joined".to_string()
            };
            (joined, by, false)
        } else {
            let written = summarize(&joined, bytes).map_err(|error| {
                format!(
                    "messages {}-{} could not be summarized: {error}",
                    left.first, right.last
                )
            })?;
            let (summary, clipped) = within(written, bytes);
            (summary, "model".to_string(), clipped)
        };
        self.keep(Node {
            id,
            first: left.first,
            last: right.last,
            from: left.from.clone(),
            to: right.to.clone(),
            summary,
            by,
            children: vec![left.id.clone(), right.id.clone()],
            clipped,
        })
    }
}
