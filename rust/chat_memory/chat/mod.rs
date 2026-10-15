//! Which chat a session writes to. A chat never ends: every session of one
//! agent appends to the same log, so a new session, a `/reset`, a resume, a
//! fork or a branch goes on from everything the agent was told before
//! instead of starting from nothing.
//!
//! A new session's chat is named, in this order, by `JEDEN_CHAT` (what a
//! launcher sets for each agent it starts), by `chatMemory.chat` in the
//! configuration, and otherwise it is the workspace's own chat, named after
//! the workspace directory and the SHA-256 of its path. A session keeps the
//! chat it started in: a resumed session, and one made from another, goes on
//! in its lineage's chat, whatever a launcher names for new sessions. A
//! delegated task (`JEDEN_TASK_SESSION`) keeps its own chat in its session
//! directory: its steps stay out of the chat that delegated it. Named chats
//! live beside the session root, in `chats/<name>/`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::log::now;
use super::Settings;

/// The variable a launcher names the chat of a new agent with.
pub const CHAT_ENV: &str = "JEDEN_CHAT";
/// The variable a delegated task's process carries.
const TASK_ENV: &str = "JEDEN_TASK_SESSION";
/// The record a chat keeps of itself.
const RECORD: &str = "chat.json";

/// One chat: its name and where it lives.
#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub name: String,
    pub dir: PathBuf,
}

impl Chat {
    /// The chat of that name under the chats root; a name that is not a chat
    /// name is refused naming `what` gave it.
    pub fn named(name: &str, what: &str) -> Result<Self, String> {
        let name = checked_name(name, what)?;
        Ok(Self {
            dir: root().join(&name),
            name,
        })
    }
}

/// What a chat records about itself when it is first written.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub name: String,
    /// The workspace the chat was started in.
    pub workspace: PathBuf,
    /// When it was started, `YYYY-MM-DDTHH:MM:SSZ`.
    pub started: String,
}

/// Where named chats live: beside the session root, so a Jeden whose
/// sessions live elsewhere (`JEDEN_SESSION_ROOT`) keeps its chats there too.
pub fn root() -> PathBuf {
    crate::session_root().with_file_name("chats")
}

/// A chat name is one plain directory name: letters, digits, `-`, `_` and
/// `.`, not starting with a dot. Anything else is refused naming `what` set it.
pub fn checked_name(name: &str, what: &str) -> Result<String, String> {
    let name = name.trim();
    let plain = name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character));
    if name.is_empty() || !plain || name.starts_with('.') {
        return Err(format!(
            "{what} names the chat `{name}`, which is not a chat name: use letters, digits, `-`, `_` and `.`, not starting with a dot"
        ));
    }
    Ok(name.to_string())
}

/// The workspace's own chat name: the directory's name and the SHA-256 of
/// its canonical path, so two workspaces of one name differ.
fn workspace_name(cwd: &Path) -> Result<String, String> {
    let canonical = cwd.canonicalize().map_err(|error| {
        format!(
            "the workspace {} cannot be resolved: {error}",
            cwd.display()
        )
    })?;
    let base: String = canonical
        .file_name()
        .ok_or_else(|| {
            format!(
                "the workspace {} is a filesystem root, which names no chat; set {CHAT_ENV} or chatMemory.chat",
                canonical.display()
            )
        })?
        .to_string_lossy()
        .chars()
        .map(|character| match character.is_ascii_alphanumeric() || "-_".contains(character) {
            true => character,
            false => '-',
        })
        .collect();
    let digest = hex::encode(Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    Ok(format!("workspace-{}-{digest}", base.trim_matches('-')))
}

/// The chat a new session at `session_dir`, working in `cwd`, starts to
/// write to.
pub fn of_session(settings: &Settings, cwd: &Path, session_dir: &Path) -> Result<Chat, String> {
    if let Some(name) = std::env::var_os(CHAT_ENV) {
        return Chat::named(&name.to_string_lossy(), CHAT_ENV);
    }
    if let Some(name) = &settings.chat {
        return Chat::named(name, "chatMemory.chat");
    }
    if std::env::var_os(TASK_ENV).is_some() {
        let name = session_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                format!(
                    "the delegated task's session {} has no name",
                    session_dir.display()
                )
            })?;
        return Ok(Chat {
            dir: session_dir.join("chat-memory"),
            name,
        });
    }
    Chat::named(&workspace_name(cwd)?, "the workspace")
}

/// The chat a session goes on in once it, or the session it was made from,
/// writes to `dir`: a session keeps its chat for its whole life, so a resume
/// or a child goes on in it. `JEDEN_CHAT` and `chatMemory.chat` name the chat
/// a new session starts in; a `JEDEN_CHAT` that is no chat name is still
/// refused, so a launcher's mistake is said at once (the configuration's is
/// refused when the settings are read).
pub fn continued(dir: PathBuf) -> Result<Chat, String> {
    if let Some(name) = std::env::var_os(CHAT_ENV) {
        checked_name(&name.to_string_lossy(), CHAT_ENV)?;
    }
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("the chat directory {} has no name", dir.display()))?;
    Ok(Chat { name, dir })
}

/// Write the chat's record the first time it is written; a record already
/// there is kept as it is.
pub fn record(chat: &Chat, cwd: &Path) -> Result<(), String> {
    let path = chat.dir.join(RECORD);
    if crate::tool_runtime::runtime_ops::platform::presence::present(&path)? {
        return Ok(());
    }
    let record = Record {
        name: chat.name.clone(),
        workspace: cwd.to_path_buf(),
        started: now()?,
    };
    let text = serde_json::to_string_pretty(&record).map_err(|error| {
        format!(
            "the chat record {} cannot be encoded: {error}",
            path.display()
        )
    })?;
    std::fs::write(&path, text + "\n").map_err(|error| {
        format!(
            "the chat record {} cannot be written: {error}",
            path.display()
        )
    })
}

/// A chat's record, none for a chat that has written none.
pub fn read_record(dir: &Path) -> Result<Option<Record>, String> {
    let path = dir.join(RECORD);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            format!(
                "the chat record {} is not a chat record ({error})",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "the chat record {} cannot be read: {error}",
            path.display()
        )),
    }
}

/// Every named chat under the chats root, by name.
pub fn list() -> Result<Vec<PathBuf>, String> {
    let root = root();
    let listing = match std::fs::read_dir(&root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "the chats directory {} cannot be read: {error}",
                root.display()
            ))
        }
    };
    let mut chats = Vec::new();
    for item in listing {
        let path = item
            .map_err(|error| {
                format!(
                    "the chats directory {} cannot be listed: {error}",
                    root.display()
                )
            })?
            .path();
        if path.is_dir() {
            chats.push(path);
        }
    }
    chats.sort();
    Ok(chats)
}

/// The chat directory `target` names: a chat of that name under the chats
/// root, else the chat the session `target` (an id or directory) wrote to.
/// Neither is refused naming both places looked at.
pub fn resolve(target: &str) -> Result<PathBuf, String> {
    let target = target.trim();
    if checked_name(target, "the request").is_ok() {
        let named = root().join(target);
        if named.is_dir() {
            return Ok(named);
        }
    }
    let session = crate::cli::sessions::session_dir_for(target);
    match super::session::session_chat(&session)? {
        Some(dir) => Ok(dir),
        None => Err(format!(
            "`{target}` is neither a chat under {} nor a session that wrote to a chat ({} records none)",
            root().display(),
            super::session::record_path(&session).display()
        )),
    }
}

/// Whether some process writes the chat now: its lock is held.
pub fn written_now(dir: &Path) -> Result<bool, String> {
    use std::os::unix::io::AsRawFd;
    let path = dir.join("owner.lock");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("{} cannot be opened: {error}", path.display())),
    };
    // SAFETY: flock on a descriptor this process owns; the shared lock taken
    // here is let go when the file closes at the end of this call.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if locked == libc::EXIT_SUCCESS {
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        true => Ok(true),
        false => Err(format!("{} cannot be examined: {error}", path.display())),
    }
}

/// A chat as `jeden chat-memory list` names it.
pub fn summary(dir: &Path) -> Result<Value, String> {
    let record = read_record(dir)?;
    let messages = super::Log::at(dir).read()?.len();
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("the chat directory {} has no name", dir.display()))?;
    Ok(serde_json::json!({
        "name": name,
        "directory": dir,
        "workspace": record.as_ref().map(|record| record.workspace.clone()),
        "started": record.map(|record| record.started),
        "messages": messages,
        "writtenNow": written_now(dir)?,
    }))
}

/// Every chat as `jeden chat-memory list` and `chat-memory/list` answer:
/// `{"chats": [...]}`, by name.
pub fn listing() -> Result<Value, String> {
    let chats = list()?
        .iter()
        .map(|dir| summary(dir))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(serde_json::json!({ "chats": chats }))
}
