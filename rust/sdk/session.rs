mod completion;
mod lifecycle;
mod turn;

use super::types::*;
use crate::cli::config::communication::{CodeFilter, DisplayPolicy};
use crate::{agent, session_conversation_turns};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};

/// Interaction and subscriber ids start at one, so none reads as unset.
static NEXT_INTERACTION_ID: AtomicU64 = AtomicU64::new(NonZeroU64::MIN.get());

struct SessionInner {
    options: SessionOptions,
    conversation: Mutex<Option<agent::Conversation>>,
    session_path: RwLock<PathBuf>,
    subscribers: Mutex<HashMap<u64, mpsc::Sender<SessionEvent>>>,
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
    /// The id `AgentSession::end_subscription` takes to end this subscription
    /// from another thread.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Blocks until the next event. Once the session drops this subscription's
    /// sender (`end_subscription`, `dispose`), the events already queued are
    /// returned and then `RecvError`, so a reader never needs a clock to stop.
    pub fn recv(&self) -> Result<SessionEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    pub fn try_recv(&self) -> Result<SessionEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        // The subscriber and active maps change by one insert or remove at a
        // time, so a holder that panicked left them whole: they are taken
        // back, where a refusal (or a skipped removal) stranded every later
        // event, subscription and abort of the session.
        if let Some(owner) = self.owner.upgrade() {
            crate::panicked::held(&owner.subscribers).remove(&self.id);
        }
    }
}

impl SessionInner {
    fn emit(&self, event: SessionEvent) -> Result<(), String> {
        let mut subscribers = crate::panicked::held(&self.subscribers);
        let mut disconnected = Vec::new();
        for (id, subscriber) in subscribers.iter() {
            if subscriber.send(event.clone()).is_err() {
                disconnected.push(*id);
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
            NEXT_INTERACTION_ID.fetch_add(NonZeroU64::MIN.get(), Ordering::Relaxed)
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
                next_subscriber: AtomicU64::new(NonZeroU64::MIN.get()),
                disposed: AtomicBool::new(false),
            }),
        }
    }

    pub fn open(options: SessionOptions, id_or_path: impl AsRef<Path>) -> Result<Self, String> {
        Self::resume(options, id_or_path)
    }

    /// Whether `id_or_path` names a stored session `resume` can open, resolved
    /// the way `resume` resolves it, so a caller can answer "not found" from
    /// the store instead of from the words of resume's error. A session that
    /// cannot be examined is the error, not "not found".
    pub fn exists(id_or_path: impl AsRef<Path>) -> Result<bool, String> {
        crate::tool_runtime::runtime_ops::platform::presence::present(&resolve_session_path(
            id_or_path.as_ref(),
        ))
    }

    pub fn resume(options: SessionOptions, id_or_path: impl AsRef<Path>) -> Result<Self, String> {
        let source = resolve_session_path(id_or_path.as_ref());
        if !crate::tool_runtime::runtime_ops::platform::presence::present(&source)? {
            return Err(format!("session not found: {}", source.display()));
        }
        crate::session_import::mark_adopted(&source)?;
        let turns = session_conversation_turns(&source)?;
        let session = Self::new(options)?;
        {
            let mut guard = session
                .inner
                .conversation
                .lock()
                .map_err(|_poisoned| "conversation lock poisoned".to_string())?;
            let conversation = guard.as_mut().ok_or("session disposed")?;
            conversation.load_history(&session.inner.options.cwd, turns, &source)?;
            *crate::panicked::written(&session.inner.session_path) = conversation.session_path();
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
        if !crate::tool_runtime::runtime_ops::platform::presence::present(
            &source.join("state.json"),
        )? {
            return Err(format!("session not found: {}", source.display()));
        }
        crate::session_import::mark_adopted(&source)?;
        let conversation = agent::Conversation::open(&options.cwd, &source)?;
        Ok(Self::from_conversation(options, conversation))
    }

    pub fn capabilities() -> Capabilities {
        Capabilities::current()
    }

    pub fn subscribe(&self) -> Result<EventSubscription, String> {
        let mut subscribers = crate::panicked::held(&self.inner.subscribers);
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        let id = self.inner.next_subscriber.fetch_add(NonZeroU64::MIN.get(), Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        subscribers.insert(id, sender);
        Ok(EventSubscription {
            id,
            receiver,
            owner: Arc::downgrade(&self.inner),
        })
    }

    /// Ends a subscription from outside the thread that reads it: its sender is
    /// dropped, so the reader's `recv` drains what is queued and then returns
    /// `RecvError`.
    pub fn end_subscription(&self, id: u64) -> Result<(), String> {
        crate::panicked::held(&self.inner.subscribers).remove(&id);
        Ok(())
    }

    pub fn set_interaction_handler(
        &self,
        handler: Option<Arc<dyn InteractionHandler>>,
    ) -> Result<(), String> {
        // Replaced whole: a writer that panicked left a handler, not a half.
        let mut interactions = crate::panicked::written(&self.inner.interactions);
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        *interactions = handler;
        Ok(())
    }

    pub fn prompt(&self, request: PromptRequest) -> Result<PromptResult, String> {
        self.dispatch_prompt(request, false)
    }

    pub fn continue_work(&self, request_id: String) -> Result<PromptResult, String> {
        self.dispatch_prompt(
            PromptRequest {
                request_id,
                prompt: "Continue retained work".into(),
                goal: None,
            },
            true,
        )
    }

    pub fn abort(&self, request_id: &str) -> Result<bool, String> {
        let active = crate::panicked::held(&self.inner.active);
        if let Some(cancel) = active.get(request_id) {
            cancel.store(true, Ordering::Release);
            crate::tool_runtime::runtime_ops::announce_cancellation();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn status(&self) -> Result<Vec<String>, String> {
        let active = crate::panicked::held(&self.inner.active);
        let mut request_ids = active.keys().cloned().collect::<Vec<_>>();
        request_ids.sort();
        Ok(request_ids)
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == NonZeroUsize::MIN.get() {
            // The last handle is going: nobody is left to be told a failed dispose.
            let _undisposed = self.dispose();
        }
    }
}

fn resolve_session_path(value: &Path) -> PathBuf {
    if value.components().count() > NonZeroUsize::MIN.get() || value.is_absolute() {
        value.to_path_buf()
    } else {
        crate::session_root().join(value)
    }
}
