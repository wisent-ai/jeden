//! The chat memory of one live conversation: what it appends to its chat's
//! log, the view it hands each turn, and the lines it builds in the
//! background between turns.
//!
//! A turn is given the conversation's system messages, the view of every
//! message of the chat before the turn as summary lines — every earlier
//! session of the chat included — and then the turn itself from the user's
//! message on, whole. Nothing else carries over between turns: the view is
//! the only continuity, so every message before the turn is summarized
//! before the turn is sent.

mod entries;
mod import;
mod recall;
mod store;

pub use import::import;
pub use recall::{date, describe, zoom};
pub use store::{record_path, session_chat};

use serde_json::{json, Value};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::chat::{self, Chat};
use super::log::now;
use super::view::{pairs, render};
use super::{fitting, settings, Log, Settings, Summarize, Tree, View};
use entries::{delegations, entries_of, is_system, is_user, own};
use store::{lineage_chat, load_taken, note_rewind, own_chat, save_taken};

/// The chat the session at `session_dir` writes to: the one it, or the
/// session it was made from, already writes to, else the one a new session
/// starts to write to.
fn chat_of(settings: &Settings, cwd: &Path, session_dir: &Path) -> Result<Chat, String> {
    match lineage_chat(session_dir)? {
        Some(dir) => chat::continued(dir),
        None => chat::of_session(settings, cwd, session_dir),
    }
}

/// The background builder and the turn share one chat's tree and view. The
/// lock guards no value, so a builder that panicked leaves nothing broken:
/// refused instead, its poison refused the memory of every later turn.
static BUSY: Mutex<()> = Mutex::new(());

fn busy() -> MutexGuard<'static, ()> {
    crate::panicked::held(&BUSY)
}

/// What a turn is told about the view, before it.
const VIEW_HEADER: &str = "Jeden's memory: the whole chat between Jeden and the user before this turn, every earlier \
session of it included, oldest first, inside <chat> tags, as one-line summaries:\n\n  id+n|text   the n messages from \
id on, summarized (newlines as spaces)\n\n\
Each message has a kind: user (the user's words), jeden (Jeden's replies), tool (Jeden's tool calls), echo (tool \
results), work (an agent's report), note (what the chat itself recorded, such as a rewind or an imported session).\n\n\
The summaries form a binary tree: each message is compressed into a line (a short message is its own line), then \
adjacent lines are merged in pairs, again and again, so recent lines cover few messages and older lines more. \
recall_conversation with id and n opens line id+n into the two lines it was made from, and for a line of a single \
message gives message id whole; with date = id it gives the date and time of message id.\n\n\
The view is your memory, and its latest word on a thing is the truth. Whenever you need any information, first find \
its latest mention in the view and zoom until you have it whole, before any other source, and before you act, guess \
or ask. Summaries keep little of tool output, so say in your reply what you learned that will matter later.\n\n";

pub struct ChatMemory {
    chat: Chat,
    session_dir: PathBuf,
    summarize: Arc<Summarize>,
    /// The first log index of each of the conversation's own messages the
    /// log holds, in the conversation's order.
    taken: Vec<u64>,
    /// The lock that makes this process the chat's one writer.
    _owner: File,
}

impl ChatMemory {
    /// The chat memory of the session at `session_dir`, working in `cwd`,
    /// given the history the conversation brings: the session's chat keeps
    /// everything it holds, and what the history says that the chat does not
    /// hold yet is appended now.
    pub fn open(
        session_dir: &Path,
        cwd: &Path,
        settings: &Settings,
        summarize: Arc<Summarize>,
        history: &[Value],
    ) -> Result<Self, String> {
        let chat = chat_of(settings, cwd, session_dir)?;
        let owner = own_chat(&chat)?;
        chat::record(&chat, cwd)?;
        let taken = load_taken(session_dir, &chat.dir)?;
        save_taken(session_dir, &chat.dir, &taken)?;
        let mut memory = Self {
            chat,
            session_dir: session_dir.to_path_buf(),
            summarize,
            taken,
            _owner: owner,
        };
        memory.sync(history)?;
        Ok(memory)
    }

    /// The chat this conversation writes to.
    pub fn chat(&self) -> &Chat {
        &self.chat
    }

    /// The conversation's history was taken back to a checkpoint
    /// (`/rewind`): the log keeps everything said, a note names what was
    /// taken back, and what is said from here on follows the note.
    pub fn replaced(&mut self, messages: &[Value]) -> Result<(), String> {
        let _guard = busy();
        let kept = own(messages).count();
        note_rewind(&self.chat.dir, &self.session_dir, &mut self.taken, kept)
    }

    /// The same note for a rewind in a process that never opened the
    /// session's memory (`jeden run "/rewind <id>"`), from the history as it
    /// stands after the rewind. A chat memory that is off, or a chat that
    /// never logged anything, has nothing to note.
    pub fn rewound_unopened(session_dir: &Path, cwd: &Path, after: &[Value]) -> Result<(), String> {
        let Some(settings) = settings(cwd)? else {
            return Ok(());
        };
        let chat = chat_of(&settings, cwd, session_dir)?;
        if !crate::tool_runtime::runtime_ops::platform::presence::present(Log::at(&chat.dir).dir())?
        {
            return Ok(());
        }
        let _owner = own_chat(&chat)?;
        let _guard = busy();
        let mut taken = load_taken(session_dir, &chat.dir)?;
        note_rewind(&chat.dir, session_dir, &mut taken, own(after).count())
    }

    /// Append the conversation's messages the log does not hold yet, word
    /// for word, flushed before the call answers.
    pub fn sync(&mut self, messages: &[Value]) -> Result<(), String> {
        let _guard = busy();
        let log = Log::at(&self.chat.dir);
        let mine: Vec<&Value> = own(messages).collect();
        // A history cut back by a path that did not say so still has its next
        // messages kept from its own length on.
        let cut = self.taken.len() > mine.len();
        self.taken.truncate(mine.len());
        let mut next = log.read()?.len() as u64;
        let mut fresh = Vec::new();
        let delegated = delegations(mine.iter().copied());
        for message in mine.iter().skip(self.taken.len()) {
            let made = entries_of(message, next, &now()?, &delegated)?;
            self.taken.push(next);
            next += made.len() as u64;
            fresh.extend(made);
        }
        if fresh.is_empty() && !cut {
            return Ok(());
        }
        log.append(&fresh)?;
        save_taken(&self.session_dir, &self.chat.dir, &self.taken)
    }

    /// The messages a turn sends: the conversation's system messages, the
    /// view of every message of the chat before the turn, then the turn
    /// itself from the user's last message on. The view is grown, merged
    /// when it passed its size, and saved first.
    pub fn outbound(&self, messages: &[Value]) -> Result<Vec<Value>, String> {
        let _guard = busy();
        let mine: Vec<&Value> = own(messages).collect();
        let logged = Log::at(&self.chat.dir).read()?;
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
        let mut tree = Tree::open(&self.chat.dir)?;
        let mut view = View::load(&self.chat.dir)?;
        view.grow(before, &mut tree, &fitting(self.summarize.as_ref())?)?;
        view.save(&self.chat.dir)?;
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
        let dir = self.chat.dir.clone();
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

/// Build every line the chat's next turn and next batch will need: the
/// line of each logged message the view does not cover, shown the view, and
/// the parent of every sibling pair in the view.
pub fn build_ahead(dir: &Path, summarize: &Summarize) -> Result<(), String> {
    let _guard = busy();
    let fitting = fitting(summarize)?;
    let logged = Log::at(dir).read()?;
    let mut tree = Tree::open(dir)?;
    let view = View::load(dir)?;
    let nodes = view.nodes(&tree)?;
    let context = render(&nodes);
    let wanted: Vec<super::tree::Wanted> = view
        .fresh(&logged)?
        .iter()
        .map(|entry| super::tree::Wanted {
            of: super::tree::Of::Message(entry),
            context: context.clone(),
        })
        .chain(pairs(&nodes))
        .collect();
    tree.build(&wanted, &fitting)
}
