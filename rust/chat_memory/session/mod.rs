//! The chat memory of one live conversation: what it appends to the log,
//! the view it hands each turn, and the lines it builds in the background
//! between turns.
//!
//! A turn is given the conversation's system messages, the view of every
//! message before the turn as summary lines, and then the turn itself from
//! the user's message on, whole. Nothing else carries over between turns:
//! the view is the only continuity, so every message before the turn is
//! summarized before the turn is sent.

mod entries;
mod recall;
mod store;

pub use recall::{date, describe, zoom};

use serde_json::{json, Value};
use std::fs::File;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::log::now;
use super::view::{render, Fitting};
use super::{
    dir, summary_attempts, summary_bytes, view_high, view_low, Log, Settings, Summarize, Tree, View,
};
use entries::{entries_of, is_system, is_user, own};
use store::{load_taken, note_rewind, own_chat, refuse_earlier_layout, save_taken};

/// The background builder and the turn share one chat's tree and view.
static BUSY: Mutex<()> = Mutex::new(());

fn busy() -> Result<MutexGuard<'static, ()>, String> {
    BUSY.lock().map_err(|_| {
        "the chat memory was poisoned by a panic in its background builder".to_string()
    })
}

/// What a turn is told about the view, before it.
const VIEW_HEADER: &str = "Jeden's memory: the whole chat between Jeden and the user before this turn, oldest first, \
inside <chat> tags, as one-line summaries:\n\n  id+n|text   the n messages from id on, summarized (newlines as spaces)\n\n\
Each message has a kind: user (the user's words), jeden (Jeden's replies), tool (Jeden's tool calls), echo (tool \
results), work (an agent's report), note (what the chat itself recorded, such as a rewind).\n\n\
The summaries form a binary tree: each message is compressed into a line (a short message is its own line), then \
adjacent lines are merged in pairs, again and again, so recent lines cover few messages and older lines more. \
recall_conversation with id and n opens line id+n into the two lines it was made from, and for a line of a single \
message gives message id whole; with date = id it gives the date and time of message id.\n\n\
The view is your memory, and its latest word on a thing is the truth. Whenever you need any information, first find \
its latest mention in the view and zoom until you have it whole, before any other source, and before you act, guess \
or ask. Summaries keep little of tool output, so say in your reply what you learned that will matter later.\n\n";

pub struct ChatMemory {
    dir: PathBuf,
    summarize: Arc<Summarize>,
    /// The first log index of each of the conversation's own messages the
    /// log holds, in the conversation's order.
    taken: Vec<u64>,
    /// The lock that makes this process the chat's one writer.
    _owner: File,
}

impl ChatMemory {
    /// The chat memory of the session at `session_dir`, given the history the
    /// conversation brings: a log that already holds the chat keeps it, and
    /// what the history says that the log does not hold is appended now.
    pub fn open(
        session_dir: &Path,
        _settings: Settings,
        summarize: Arc<Summarize>,
        history: &[Value],
    ) -> Result<Self, String> {
        let dir = dir(session_dir);
        refuse_earlier_layout(&dir)?;
        let owner = own_chat(&dir)?;
        let taken = load_taken(&dir)?;
        let mut memory = Self {
            dir,
            summarize,
            taken,
            _owner: owner,
        };
        memory.sync(history)?;
        Ok(memory)
    }

    /// The conversation's history was taken back to a checkpoint
    /// (`/rewind`): the log keeps everything said, a note names what was
    /// taken back, and what is said from here on follows the note.
    pub fn replaced(&mut self, messages: &[Value]) -> Result<(), String> {
        let _guard = busy()?;
        let kept = own(messages).count();
        note_rewind(&self.dir, &mut self.taken, kept)
    }

    /// The same note for a rewind in a process that never opened the
    /// session's memory (`jeden run "/rewind <id>"`), from the history as it
    /// stands after the rewind. A session the memory never logged has
    /// nothing to note.
    pub fn rewound_unopened(session_dir: &Path, after: &[Value]) -> Result<(), String> {
        let dir = dir(session_dir);
        if !Log::at(&dir).dir().exists() {
            return Ok(());
        }
        let _owner = own_chat(&dir)?;
        let _guard = busy()?;
        let mut taken = load_taken(&dir)?;
        note_rewind(&dir, &mut taken, own(after).count())
    }

    /// Append the conversation's messages the log does not hold yet, word
    /// for word, flushed before the call answers.
    pub fn sync(&mut self, messages: &[Value]) -> Result<(), String> {
        let _guard = busy()?;
        let log = Log::at(&self.dir);
        let mine: Vec<&Value> = own(messages).collect();
        // A history cut back by a path that did not say so still has its next
        // messages kept from its own length on.
        self.taken.truncate(mine.len());
        let mut next = log.read()?.len() as u64;
        let mut fresh = Vec::new();
        for message in mine.iter().skip(self.taken.len()) {
            let made = entries_of(message, next, &now()?)?;
            self.taken.push(next);
            next += made.len() as u64;
            fresh.extend(made);
        }
        if fresh.is_empty() {
            return Ok(());
        }
        log.append(&fresh)?;
        save_taken(&self.dir, &self.taken)
    }

    /// The messages a turn sends: the conversation's system messages, the
    /// view of every message before the turn, then the turn itself from the
    /// user's last message on. The view is grown, merged when it passed its
    /// size, and saved first.
    pub fn outbound(&self, messages: &[Value]) -> Result<Vec<Value>, String> {
        let _guard = busy()?;
        let mine: Vec<&Value> = own(messages).collect();
        let logged = Log::at(&self.dir).read()?;
        // The turn starts at the user's last message; a conversation with
        // none is all turn, and every message the log holds comes before it.
        let (turn, start) = match mine.iter().rposition(|message| is_user(message)) {
            Some(turn) => {
                let start = self.taken.get(turn).ok_or_else(|| {
                    format!("the chat log does not hold the turn's message {turn} yet")
                })?;
                let start = usize::try_from(*start).map_err(|error| {
                    format!("the turn starts at a log index this machine cannot count: {error}")
                })?;
                (turn, start)
            }
            None => (mine.len(), logged.len()),
        };
        let before = logged.get(..start).ok_or_else(|| {
            format!(
                "the turn starts at message {start}, but the chat log holds {}",
                logged.len()
            )
        })?;
        let mut tree = Tree::open(&self.dir)?;
        let mut view = View::load(&self.dir)?;
        let fitting = Fitting {
            summary_bytes: summary_bytes(),
            attempts: summary_attempts(),
            high: view_high(),
            low: view_low(),
            summarize: self.summarize.as_ref(),
        };
        view.grow(before, &mut tree, &fitting)?;
        view.save(&self.dir)?;
        let mut out: Vec<Value> = messages
            .iter()
            .filter(|message| is_system(message))
            .cloned()
            .collect();
        let nodes = view.nodes(&tree)?;
        if !nodes.is_empty() {
            out.push(json!({
                "role": "system",
                "content": format!("{VIEW_HEADER}{}", render(&nodes)),
            }));
        }
        out.extend(mine.into_iter().skip(turn).cloned());
        Ok(out)
    }

    /// Between turns, build ahead the line of every message the view does
    /// not cover yet and the parent of every sibling pair in the view, so
    /// the next turn and the next batch ask no model while they wait. Runs
    /// on its own thread; a failure is said on standard error with its
    /// cause, and the turn that needs the line builds it then.
    pub fn build_in_background(&self) {
        let dir = self.dir.clone();
        let summarize = Arc::clone(&self.summarize);
        let spawned = std::thread::Builder::new()
            .name("chat-memory-builder".into())
            .spawn(move || {
                if let Err(error) = build_ahead(&dir, summarize.as_ref()) {
                    eprintln!("jeden: the chat memory's lines were not built ahead: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("jeden: the chat memory's background builder could not start: {error}");
        }
    }
}

fn build_ahead(dir: &Path, summarize: &Summarize) -> Result<(), String> {
    let _guard = busy()?;
    let logged = Log::at(dir).read()?;
    let mut tree = Tree::open(dir)?;
    let view = View::load(dir)?;
    let nodes = view.nodes(&tree)?;
    let context = render(&nodes);
    let covered = usize::try_from(view.covered()).map_err(|error| {
        format!("the view covers more messages than this machine counts: {error}")
    })?;
    for entry in logged.iter().skip(covered) {
        tree.leaf(
            entry,
            &context,
            summary_bytes(),
            summary_attempts(),
            summarize,
        )?;
    }
    let pairs = nodes.windows(NonZeroUsize::MIN.get() + NonZeroUsize::MIN.get());
    for (at, window) in pairs.enumerate() {
        let [left, right] = window else {
            continue;
        };
        let (l, i) = (
            left.l + NonZeroU32::MIN.get(),
            left.i >> NonZeroU32::MIN.get(),
        );
        if super::tree::halves(l, i) == Some(((left.l, left.i), (right.l, right.i))) {
            let upto = render(&nodes[..=at + NonZeroUsize::MIN.get()]);
            tree.parent(l, i, &upto, summary_bytes(), summary_attempts(), summarize)?;
        }
    }
    Ok(())
}
