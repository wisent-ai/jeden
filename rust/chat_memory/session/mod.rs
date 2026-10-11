//! The chat memory of one live conversation: what it appends to the log,
//! the view it hands each turn, and the summaries it builds in the
//! background between turns.

mod recall;

pub use recall::{dates, describe, zoom};

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{dir, summary_bytes, Entry, Log, Settings, Summarize, Tree, View};

/// One process works on a chat memory at a time: the background builder and
/// the turn share its tree and its view.
static BUSY: Mutex<()> = Mutex::new(());

fn busy() -> Result<MutexGuard<'static, ()>, String> {
    BUSY.lock().map_err(|_| {
        "the chat memory was poisoned by a panic in its background builder".to_string()
    })
}

fn is_system(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("system")
}

pub struct ChatMemory {
    dir: PathBuf,
    settings: Settings,
    summarize: Arc<Summarize>,
    /// How many of the conversation's own messages (all but system ones) the
    /// log already holds.
    synced: usize,
}

impl ChatMemory {
    /// The chat memory of the session at `session_dir`, given the history the
    /// conversation brings. A log that already holds the chat keeps it as it
    /// is; a log the session never had takes that history now, so a session
    /// the memory is turned on for keeps everything said before.
    pub fn open(
        session_dir: &Path,
        settings: Settings,
        summarize: Arc<Summarize>,
        history: &[Value],
    ) -> Result<Self, String> {
        let dir = dir(session_dir);
        let held = Log::at(&dir).read()?.len();
        let own = history.iter().filter(|message| !is_system(message)).count();
        let mut memory = Self {
            dir,
            settings,
            summarize,
            synced: own.min(held),
        };
        memory.sync(history)?;
        Ok(memory)
    }

    /// Append the conversation's messages the log does not hold yet, each
    /// as the conversation holds it.
    pub fn sync(&mut self, messages: &[Value]) -> Result<(), String> {
        let log = Log::at(&self.dir);
        let _guard = busy()?;
        let next = log.read()?.len() as u64;
        let mut fresh = Vec::new();
        for (message, index) in messages
            .iter()
            .filter(|message| !is_system(message))
            .skip(self.synced)
            .zip(next..)
        {
            let role = message.get("role").and_then(Value::as_str).ok_or_else(|| {
                format!("message {index} of the conversation names no role, so the chat log cannot keep it")
            })?;
            let mut fields = message.as_object().cloned().ok_or_else(|| {
                format!("message {index} of the conversation is not an object, so the chat log cannot keep it")
            })?;
            fields.remove("role");
            // An assistant turn that only called tools carries no content.
            let content = fields.remove("content");
            fresh.push(Entry {
                index,
                at: crate::agent::now_stamp(),
                role: role.to_string(),
                content: match content {
                    Some(content) => content,
                    None => Value::Null,
                },
                fields,
            });
        }
        log.append(&fresh)?;
        self.synced += fresh.len();
        Ok(())
    }

    /// The messages a turn sends: the conversation's system messages, the
    /// frontier of the view as one memory message, then the tail word for
    /// word. The view is fitted to its size first and kept.
    pub fn outbound(&self, messages: &[Value]) -> Result<Vec<Value>, String> {
        let _guard = busy()?;
        let entries = Log::at(&self.dir).read()?;
        let mut tree = Tree::open(&self.dir)?;
        let mut view = View::load(&self.dir)?;
        view.fit(
            &entries,
            &mut tree,
            self.settings.view_bytes,
            summary_bytes(),
            self.summarize.as_ref(),
        )?;
        view.save(&self.dir)?;
        let mut out: Vec<Value> = messages
            .iter()
            .filter(|message| is_system(message))
            .cloned()
            .collect();
        let nodes = view.nodes(&tree)?;
        if !nodes.is_empty() {
            let mut block = String::from(
                "Conversation memory: every message of this chat before the ones that follow, oldest first, \
                 as summaries. Each line names a node (`first-last` message, entered between two times in \
                 seconds since 1970). recall_conversation with node=<id> answers a node's two halves, or a \
                 message word for word; with from and to (seconds since 1970) it answers the messages of that time.\n",
            );
            for node in &nodes {
                block.push_str(&node.line());
            }
            out.push(json!({ "role": "system", "content": block }));
        }
        for entry in view.tail(&entries)? {
            out.push(entry.message());
        }
        Ok(out)
    }

    /// Between turns, write the leaf of every tail message the next fold
    /// takes (`view::foldable`), so
    /// the next fold of the view asks no model while a turn waits. Runs on its
    /// own thread; a failure is said on standard error with its cause, and
    /// the turn that needs the leaf writes it then.
    pub fn build_in_background(&self) {
        let dir = self.dir.clone();
        let summarize = Arc::clone(&self.summarize);
        let spawned = std::thread::Builder::new()
            .name("chat-memory-builder".into())
            .spawn(move || {
                if let Err(error) = build_leaves(&dir, summarize.as_ref()) {
                    eprintln!("jeden: the chat memory's summaries were not built ahead: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("jeden: the chat memory's background builder could not start: {error}");
        }
    }
}

fn build_leaves(dir: &Path, summarize: &Summarize) -> Result<(), String> {
    let _guard = busy()?;
    let entries = Log::at(dir).read()?;
    let mut tree = Tree::open(dir)?;
    let view = View::load(dir)?;
    for entry in super::super::view::foldable(view.tail(&entries)?) {
        tree.leaf(entry, summary_bytes(), summarize)?;
    }
    Ok(())
}
