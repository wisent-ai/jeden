mod turn;
mod completion;

use super::types::*;
use crate::cli::config::communication::{CodeFilter, DisplayPolicy};
use crate::{agent, session_conversation_turns, Args};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::time::Duration;

const EVENT_BUFFER: usize = 1024;
static NEXT_INTERACTION_ID: AtomicU64 = AtomicU64::new(1);

struct SessionInner {
    options: SessionOptions,
    conversation: Mutex<Option<agent::Conversation>>,
    session_path: RwLock<PathBuf>,
    subscribers: Mutex<HashMap<u64, mpsc::SyncSender<SessionEvent>>>,
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
    interactions: RwLock<Option<Arc<dyn InteractionHandler>>>,
    next_subscriber: AtomicU64,
    disposed: AtomicBool,
}

#[derive(Clone)]
pub struct AgentSession {
    inner: Arc<SessionInner>,
}

pub struct EventSubscription {
    id: u64,
    receiver: mpsc::Receiver<SessionEvent>,
    owner: Weak<SessionInner>,
}

impl EventSubscription {
    pub fn recv(&self) -> Result<SessionEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<SessionEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<SessionEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            if let Ok(mut subscribers) = owner.subscribers.lock() {
                subscribers.remove(&self.id);
            }
        }
    }
}

impl SessionInner {
    fn emit(&self, event: SessionEvent) -> Result<(), String> {
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| "event subscription lock poisoned".to_string())?;
        let mut disconnected = Vec::new();
        for (id, subscriber) in subscribers.iter() {
            match subscriber.try_send(event.clone()) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => disconnected.push(*id),
                Err(mpsc::TrySendError::Full(_)) => {
                    return Err(format!("event subscriber {} is not consuming events", id));
                }
            }
        }
        for id in disconnected {
            subscribers.remove(&id);
        }
        Ok(())
    }

    fn interaction_token(&self, prefix: &str) -> String {
        format!(
            "{}-{}",
            prefix,
            NEXT_INTERACTION_ID.fetch_add(1, Ordering::Relaxed)
        )
    }
}

impl AgentSession {
    pub fn new(options: SessionOptions) -> Result<Self, String> {
        let conversation = agent::Conversation::new(&options.cwd)?;
        Ok(Self::from_conversation(options, conversation))
    }

    fn from_conversation(options: SessionOptions, conversation: agent::Conversation) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                options,
                session_path: RwLock::new(conversation.session_path()),
                conversation: Mutex::new(Some(conversation)),
                subscribers: Mutex::new(HashMap::new()),
                active: Mutex::new(HashMap::new()),
                interactions: RwLock::new(None),
                next_subscriber: AtomicU64::new(1),
                disposed: AtomicBool::new(false),
            }),
        }
    }

    pub fn open(options: SessionOptions, id_or_path: impl AsRef<Path>) -> Result<Self, String> {
        Self::resume(options, id_or_path)
    }

    pub fn resume(options: SessionOptions, id_or_path: impl AsRef<Path>) -> Result<Self, String> {
        let source = resolve_session_path(id_or_path.as_ref());
        if !source.exists() {
            return Err(format!("session not found: {}", source.display()));
        }
        let turns = session_conversation_turns(&source)?;
        let session = Self::new(options)?;
        {
            let mut guard = session
                .inner
                .conversation
                .lock()
                .map_err(|_| "conversation lock poisoned".to_string())?;
            let conversation = guard.as_mut().ok_or("session disposed")?;
            conversation.load_history(&session.inner.options.cwd, turns, &source)?;
            *session.inner.session_path.write().map_err(|_| "session path lock poisoned")? = conversation.session_path();
        }
        Ok(session)
    }

    /// Continue an existing ledger in place: the same turns as `resume`, but
    /// subsequent turns append to `id_or_path` itself instead of seeding a new
    /// session directory. This is what lets a headless client keep writing to
    /// the ledger the operator's own terminal opened.
    pub fn resume_in_place(
        options: SessionOptions,
        id_or_path: impl AsRef<Path>,
    ) -> Result<Self, String> {
        let source = resolve_session_path(id_or_path.as_ref());
        if !source.join("state.json").is_file() {
            return Err(format!("session not found: {}", source.display()));
        }
        let conversation = agent::Conversation::open(&options.cwd, &source)?;
        Ok(Self::from_conversation(options, conversation))
    }

    pub fn capabilities() -> Capabilities {
        Capabilities::current()
    }

    pub fn subscribe(&self) -> Result<EventSubscription, String> {
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        let id = self.inner.next_subscriber.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(EVENT_BUFFER);
        self.inner
            .subscribers
            .lock()
            .map_err(|_| "event subscription lock poisoned".to_string())?
            .insert(id, sender);
        Ok(EventSubscription {
            id,
            receiver,
            owner: Arc::downgrade(&self.inner),
        })
    }

    pub fn set_interaction_handler(
        &self,
        handler: Option<Arc<dyn InteractionHandler>>,
    ) -> Result<(), String> {
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        *self
            .inner
            .interactions
            .write()
            .map_err(|_| "interaction handler lock poisoned".to_string())? = handler;
        Ok(())
    }

    pub fn prompt(&self, request: PromptRequest) -> Result<PromptResult, String> {
        self.dispatch_prompt(request, false)
    }

    pub fn continue_work(&self, request_id: String) -> Result<PromptResult, String> {
        self.dispatch_prompt(PromptRequest {
            request_id,
            prompt: "Continue retained work".into(),
            goal: None,
        }, true)
    }



    pub fn abort(&self, request_id: &str) -> Result<bool, String> {
        let active = self
            .inner
            .active
            .lock()
            .map_err(|_| "active request lock poisoned")?;
        if let Some(cancel) = active.get(request_id) {
            cancel.store(true, Ordering::Release);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn status(&self) -> Result<Vec<String>, String> {
        let active = self
            .inner
            .active
            .lock()
            .map_err(|_| "active request lock poisoned")?;
        let mut request_ids = active.keys().cloned().collect::<Vec<_>>();
        request_ids.sort();
        Ok(request_ids)
    }


    pub fn dispose(&self) -> Result<(), String> {
        if self.inner.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        for cancel in self
            .inner
            .active
            .lock()
            .map_err(|_| "active request lock poisoned")?
            .values()
        {
            cancel.store(true, Ordering::Release);
        }
        self.inner
            .subscribers
            .lock()
            .map_err(|_| "event subscription lock poisoned")?
            .clear();
        *self
            .inner
            .interactions
            .write()
            .map_err(|_| "interaction handler lock poisoned")? = None;
        *self
            .inner
            .conversation
            .lock()
            .map_err(|_| "conversation lock poisoned")? = None;
        Ok(())
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.dispose();
        }
    }
}

fn resolve_session_path(value: &Path) -> PathBuf {
    if value.components().count() > 1 || value.is_absolute() {
        value.to_path_buf()
    } else {
        crate::session_root().join(value)
    }
}

