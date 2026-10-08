use crate::task_runtime::types::{MailMessage, TaskError};
use crate::task_runtime::{atomic_json, next_sequence, now_millis};
use std::fs;
use std::path::{Path, PathBuf};

/// The delegated agents' inboxes. A message waits until its agent reads it;
/// no count of waiting messages and no message length is chosen here.
#[derive(Clone, Debug)]
pub struct Mailbox {
    root: PathBuf,
}

impl Mailbox {
    pub fn new(store: &Path) -> Result<Self, TaskError> {
        let root = store.join("mailboxes");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    fn agent_dir(&self, agent: &str) -> Result<PathBuf, TaskError> {
        if agent.is_empty()
            || agent.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        {
            return Err(TaskError::Invalid("invalid mailbox agent id".into()));
        }
        Ok(self.root.join(agent))
    }
    pub fn send(
        &self,
        from: &str,
        to: &str,
        body: &str,
        correlation_id: Option<String>,
        reply_to: Option<String>,
    ) -> Result<MailMessage, TaskError> {
        if body.is_empty() {
            return Err(TaskError::Invalid("mail body must not be empty".into()));
        }
        let dir = self.agent_dir(to)?;
        fs::create_dir_all(&dir)?;
        let at = now_millis();
        let id = format!("msg-{at}-{}-{}", std::process::id(), next_sequence());
        let message = MailMessage {
            id: id.clone(),
            from: from.into(),
            to: to.into(),
            body: body.into(),
            correlation_id,
            reply_to,
            created_at: at,
            delivered_at: None,
        };
        atomic_json(&dir.join(format!("{id}.json")), &message)?;
        atomic_json(
            &self.root.join(format!("{to}.wake.json")),
            &serde_json::json!({"agent": to, "at": at, "message": id}),
        )?;
        Ok(message)
    }
    /// The agent's waiting messages, oldest first. With `deliver`, each is
    /// stamped delivered, handed back this once and leaves the inbox, so the
    /// inbox holds only what is still unread and needs no size of its own.
    pub fn inbox(&self, agent: &str, deliver: bool) -> Result<Vec<MailMessage>, TaskError> {
        let dir = self.agent_dir(agent)?;
        let mut messages = Vec::new();
        if !dir.exists() {
            return Ok(messages);
        }
        let mut paths = fs::read_dir(&dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|v| v.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let mut message: MailMessage = serde_json::from_slice(&fs::read(&path)?)?;
            if deliver {
                if message.delivered_at.is_none() {
                    message.delivered_at = Some(now_millis());
                }
                fs::remove_file(&path)?;
            }
            messages.push(message);
        }
        if deliver && !messages.is_empty() {
            let _ = fs::remove_file(self.root.join(format!("{agent}.wake.json")));
        }
        Ok(messages)
    }
    /// Wait for a message. What ends this is a message arriving — the peer
    /// answering is the event this call is about. The mailbox directory is
    /// watched before it is read, so a message landing between the read and
    /// the wait still wakes it.
    pub fn wait(
        &self,
        agent: &str,
        correlation: Option<&str>,
    ) -> Result<Vec<MailMessage>, TaskError> {
        let dir = self.agent_dir(agent)?;
        fs::create_dir_all(&dir)?;
        let watch = crate::task_runtime::watch::watch(&dir)?;
        loop {
            // Only the messages this wait is for are delivered; the rest stay
            // in the inbox for their own reader.
            let mut found = self
                .inbox(agent, false)?
                .into_iter()
                .filter(|m| {
                    correlation.is_none_or(|id| {
                        m.correlation_id.as_deref() == Some(id) || m.reply_to.as_deref() == Some(id)
                    })
                })
                .collect::<Vec<_>>();
            if !found.is_empty() {
                for message in &mut found {
                    if message.delivered_at.is_none() {
                        message.delivered_at = Some(now_millis());
                    }
                    fs::remove_file(dir.join(format!("{}.json", message.id)))?;
                }
                return Ok(found);
            }
            watch.wait()?;
        }
    }
    pub fn wake_pending(&self, agent: &str) -> Result<bool, TaskError> {
        Ok(self.root.join(format!("{agent}.wake.json")).exists())
    }
}
