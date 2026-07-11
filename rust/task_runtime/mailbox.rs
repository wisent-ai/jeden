use super::types::{MailMessage, TaskError};
use super::{atomic_json, next_sequence, now_millis};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Mailbox { root: PathBuf, max_messages: usize }

impl Mailbox {
    pub fn new(store: &Path, max_messages: usize) -> Result<Self, TaskError> {
        let root = store.join("mailboxes"); fs::create_dir_all(&root)?;
        Ok(Self { root, max_messages: max_messages.max(1) })
    }
    fn agent_dir(&self, agent: &str) -> Result<PathBuf, TaskError> {
        if agent.is_empty() || agent.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) { return Err(TaskError::Invalid("invalid mailbox agent id".into())); }
        Ok(self.root.join(agent))
    }
    pub fn send(&self, from: &str, to: &str, body: &str, correlation_id: Option<String>, reply_to: Option<String>) -> Result<MailMessage, TaskError> {
        if body.is_empty() || body.len() > 64 * 1024 { return Err(TaskError::Invalid("mail body must contain 1..65536 bytes".into())); }
        let dir = self.agent_dir(to)?; fs::create_dir_all(&dir)?;
        let mut paths = fs::read_dir(&dir)?.flatten().map(|entry| entry.path()).filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json")).collect::<Vec<_>>();
        paths.sort();
        if paths.len() >= self.max_messages {
            for path in &paths {
                let delivered = fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<MailMessage>(&bytes).ok()).and_then(|message| message.delivered_at).is_some();
                if delivered { let _ = fs::remove_file(path); }
                if fs::read_dir(&dir)?.count() < self.max_messages { break; }
            }
        }
        let count = fs::read_dir(&dir)?.count();
        if count >= self.max_messages { return Err(TaskError::Capacity { running: count, limit: self.max_messages }); }
        let at = now_millis();
        let id = format!("msg-{at}-{}-{}", std::process::id(), next_sequence());
        let message = MailMessage { id: id.clone(), from: from.into(), to: to.into(), body: body.into(), correlation_id, reply_to, created_at: at, delivered_at: None };
        atomic_json(&dir.join(format!("{id}.json")), &message)?;
        atomic_json(&self.root.join(format!("{to}.wake.json")), &serde_json::json!({"agent": to, "at": at, "message": id}))?;
        Ok(message)
    }
    pub fn inbox(&self, agent: &str, deliver: bool) -> Result<Vec<MailMessage>, TaskError> {
        let dir = self.agent_dir(agent)?;
        let mut messages = Vec::new();
        if !dir.exists() { return Ok(messages); }
        let mut paths = fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|v| v.to_str()) == Some("json")).collect::<Vec<_>>(); paths.sort();
        for path in paths.into_iter().take(self.max_messages) {
            let mut message: MailMessage = serde_json::from_slice(&fs::read(&path)?)?;
            if deliver && message.delivered_at.is_none() { message.delivered_at = Some(now_millis()); atomic_json(&path, &message)?; }
            messages.push(message);
        }
        if deliver && !messages.is_empty() {
            let _ = fs::remove_file(self.root.join(format!("{agent}.wake.json")));
        }
        Ok(messages)
    }
    pub fn wait(&self, agent: &str, correlation: Option<&str>, timeout: Duration) -> Result<Vec<MailMessage>, TaskError> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = self.inbox(agent, true)?.into_iter().filter(|m| correlation.map_or(true, |id| m.correlation_id.as_deref() == Some(id) || m.reply_to.as_deref() == Some(id))).collect::<Vec<_>>();
            if !found.is_empty() { return Ok(found); }
            if Instant::now() >= deadline { return Err(TaskError::Timeout(format!("mailbox wait timed out for {agent}"))); }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn wake_pending(&self, agent: &str) -> Result<bool, TaskError> { Ok(self.root.join(format!("{agent}.wake.json")).exists()) }
}
