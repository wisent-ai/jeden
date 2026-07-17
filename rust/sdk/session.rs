use super::types::*;
use crate::{agent, session_conversation_turns, Args};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock, Weak};
use std::time::Duration;

const EVENT_BUFFER: usize = 1024;
static NEXT_INTERACTION_ID: AtomicU64 = AtomicU64::new(1);

struct SessionInner {
    options: SessionOptions,
    conversation: Mutex<Option<agent::Conversation>>,
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
        Ok(Self {
            inner: Arc::new(SessionInner {
                options,
                conversation: Mutex::new(Some(conversation)),
                subscribers: Mutex::new(HashMap::new()),
                active: Mutex::new(HashMap::new()),
                interactions: RwLock::new(None),
                next_subscriber: AtomicU64::new(1),
                disposed: AtomicBool::new(false),
            }),
        })
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
            conversation.load_history(&session.inner.options.cwd, turns)?;
        }
        Ok(session)
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
        if request.request_id.trim().is_empty() {
            return Err("request_id must not be empty".into());
        }
        if request.prompt.trim().is_empty() {
            return Err("prompt must not be empty".into());
        }
        if self.inner.disposed.load(Ordering::Acquire) {
            return Err("session disposed".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| "active request lock poisoned")?;
            if active.contains_key(&request.request_id) {
                return Err(format!("request already active: {}", request.request_id));
            }
            active.insert(request.request_id.clone(), cancel.clone());
        }
        let result = self.run_prompt(&request, cancel);
        if let Ok(mut active) = self.inner.active.lock() {
            active.remove(&request.request_id);
        }
        if let Err(message) = &result {
            let _ = self.inner.emit(SessionEvent {
                request_id: request.request_id,
                event: SessionEventKind::Error {
                    message: message.clone(),
                },
            });
        }
        result
    }

    fn run_prompt(
        &self,
        request: &PromptRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<PromptResult, String> {
        let mut conversation_guard = self
            .inner
            .conversation
            .lock()
            .map_err(|_| "conversation lock poisoned".to_string())?;
        let conversation = conversation_guard.as_mut().ok_or("session disposed")?;
        let args = args_from_options(&self.inner.options, request.prompt.clone());
        let event_error = Arc::new(Mutex::new(None::<String>));
        let request_id = request.request_id.clone();

        let progress_inner = self.inner.clone();
        let progress_id = request_id.clone();
        let progress_error = event_error.clone();
        let stream_inner = self.inner.clone();
        let stream_id = request_id.clone();
        let stream_error = event_error.clone();
        let ask_inner = self.inner.clone();
        let ask_id = request_id.clone();
        let approve_inner = self.inner.clone();
        let approve_id = request_id.clone();
        let approve_error = event_error.clone();

        let mut hooks = agent::RunHooks {
            cancel,
            interactive: false,
            progress: Box::new(move |message| {
                if let Err(error) = progress_inner.emit(SessionEvent {
                    request_id: progress_id.clone(),
                    event: SessionEventKind::Status {
                        message: message.to_string(),
                    },
                }) {
                    if let Ok(mut slot) = progress_error.lock() {
                        *slot = Some(error);
                    }
                }
            }),
            stream: Box::new(move |text| {
                if let Err(error) = stream_inner.emit(SessionEvent {
                    request_id: stream_id.clone(),
                    event: SessionEventKind::TextDelta {
                        text: text.to_string(),
                    },
                }) {
                    if let Ok(mut slot) = stream_error.lock() {
                        *slot = Some(error);
                    }
                }
            }),
            ask_user: Some(Box::new(move |question, options| {
                let token = ask_inner.interaction_token("elicit");
                ask_inner.emit(SessionEvent {
                    request_id: ask_id.clone(),
                    event: SessionEventKind::Elicitation {
                        token: token.clone(),
                        question: question.to_string(),
                        options: options.to_vec(),
                    },
                })?;
                let handler = ask_inner
                    .interactions
                    .read()
                    .map_err(|_| "interaction handler lock poisoned")?
                    .clone();
                handler
                    .ok_or("elicitation requires an interaction handler")?
                    .elicit(ElicitationRequest {
                        token,
                        request_id: ask_id.clone(),
                        question: question.to_string(),
                        options: options.to_vec(),
                    })
            })),
            approve: Box::new(move |tool, detail| {
                let token = approve_inner.interaction_token("approval");
                if let Err(error) = approve_inner.emit(SessionEvent {
                    request_id: approve_id.clone(),
                    event: SessionEventKind::Approval {
                        token: token.clone(),
                        tool: tool.to_string(),
                        detail: detail.to_string(),
                    },
                }) {
                    if let Ok(mut slot) = approve_error.lock() {
                        *slot = Some(error);
                    }
                    return false;
                }
                let result = approve_inner
                    .interactions
                    .read()
                    .map_err(|_| "interaction handler lock poisoned".to_string())
                    .and_then(|guard| {
                        guard
                            .clone()
                            .ok_or_else(|| "approval requires an interaction handler".to_string())
                    })
                    .and_then(|handler| {
                        handler.approve(ApprovalRequest {
                            token,
                            request_id: approve_id.clone(),
                            tool: tool.to_string(),
                            detail: detail.to_string(),
                        })
                    });
                match result {
                    Ok(approved) => approved,
                    Err(error) => {
                        if let Ok(mut slot) = approve_error.lock() {
                            *slot = Some(error);
                        }
                        false
                    }
                }
            }),
        };
        let text = conversation.run_turn(&args, &request.prompt, &[], &mut hooks)?;
        if let Some(error) = event_error
            .lock()
            .map_err(|_| "event error lock poisoned")?
            .take()
        {
            return Err(error);
        }
        let session_path = conversation.session_path();
        self.inner.emit(SessionEvent {
            request_id: request_id.clone(),
            event: SessionEventKind::Result { text: text.clone() },
        })?;
        Ok(PromptResult {
            request_id,
            text,
            session_path,
        })
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

    pub fn session_path(&self) -> Result<PathBuf, String> {
        let guard = self
            .inner
            .conversation
            .lock()
            .map_err(|_| "conversation lock poisoned")?;
        Ok(guard.as_ref().ok_or("session disposed")?.session_path())
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

fn args_from_options(options: &SessionOptions, prompt: String) -> Args {
    Args {
        command: "run".into(),
        cwd: options.cwd.clone(),
        model: options.model.clone(),
        max_tokens: options.max_tokens,
        max_steps: options.max_steps,
        allow_write: options.allow_write || options.auto_approve,
        allow_command: options.allow_command || options.auto_approve,
        yolo: options.auto_approve,
        model_only: false,
        json: false,
        resume_session: None,
        positionals: vec![prompt],
    }
}
