use super::protocol::{
    Envelope, ErrorEnvelope, EventEnvelope, RequestEnvelope, RequestMeta, ValidationError,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;
use tokio_stream::{wrappers::ReceiverStream, Stream};

const DEFAULT_EVENT_BUFFER: usize = 256;
const CANCEL_METHOD: &str = "request.cancel";

/// An I/O failure reported by an injected session transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    message: String,
}

impl TransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransportError {}

/// Object-safe, asynchronous envelope transport injected into [`SessionClient`].
///
/// Implementations must permit one `recv` to be in flight concurrently with `send` calls.
pub trait SessionTransport: Send + Sync + 'static {
    fn send(&self, envelope: Envelope) -> BoxFuture<'_, Result<(), TransportError>>;
    fn recv(&self) -> BoxFuture<'_, Result<Envelope, TransportError>>;
}

/// A failure produced by the asynchronous session client.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    Validation(ValidationError),
    Transport(TransportError),
    Protocol(ErrorEnvelope),
    DuplicateRequestId(String),
    Disposed,
    EventStreamLagged,
    UnexpectedEnvelope(&'static str),
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => write!(formatter, "invalid request: {error}"),
            Self::Transport(error) => write!(formatter, "transport failed: {error}"),
            Self::Protocol(error) => write!(
                formatter,
                "protocol error {}: {}",
                error.error.code, error.error.message
            ),
            Self::DuplicateRequestId(id) => write!(formatter, "duplicate request id: {id}"),
            Self::Disposed => formatter.write_str("session client is disposed"),
            Self::EventStreamLagged => formatter.write_str("event subscriber lagged"),
            Self::UnexpectedEnvelope(kind) => {
                write!(formatter, "unexpected inbound {kind} envelope")
            }
        }
    }
}

impl Error for ClientError {}

impl From<ValidationError> for ClientError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

struct EventSubscriber {
    events: mpsc::Sender<Result<EventEnvelope, ClientError>>,
    terminal: Option<oneshot::Sender<ClientError>>,
}

struct ClientInner {
    transport: Arc<dyn SessionTransport>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, ClientError>>>>,
    subscribers: Mutex<HashMap<u64, EventSubscriber>>,
    cursors: Mutex<HashMap<String, String>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    next_request_id: AtomicU64,
    next_subscriber_id: AtomicU64,
    event_buffer: usize,
    disposed: AtomicBool,
    terminated: AtomicBool,
    terminated_notify: Notify,
}

impl ClientInner {
    fn terminate(&self, error: ClientError) {
        self.disposed.store(true, Ordering::Release);

        let pending = {
            let mut pending = self.pending.lock().unwrap_or_else(|lock| lock.into_inner());
            std::mem::take(&mut *pending)
        };
        for sender in pending.into_values() {
            let _ = sender.send(Err(error.clone()));
        }

        let subscribers = {
            let mut subscribers = self
                .subscribers
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            std::mem::take(&mut *subscribers)
        };
        for mut subscriber in subscribers.into_values() {
            if let Some(sender) = subscriber.terminal.take() {
                let _ = sender.send(error.clone());
            }
        }
        self.terminated.store(true, Ordering::Release);
        self.terminated_notify.notify_waiters();
    }

    fn complete(&self, id: &str, result: Result<Value, ClientError>) -> bool {
        let sender = self
            .pending
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(id);
        match sender {
            Some(sender) => {
                let _ = sender.send(result);
                true
            }
            None => false,
        }
    }

    fn publish(&self, event: EventEnvelope) {
        self.cursors
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .insert(event.session_id.clone(), event.cursor.clone());

        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        subscribers.retain(|_, subscriber| {
            if subscriber.events.try_send(Ok(event.clone())).is_ok() {
                true
            } else {
                if let Some(sender) = subscriber.terminal.take() {
                    let _ = sender.send(ClientError::EventStreamLagged);
                }
                false
            }
        });
    }
}

/// Cloneable asynchronous client for `jeden.session.v1`.
#[derive(Clone)]
pub struct SessionClient {
    inner: Arc<ClientInner>,
}

impl SessionClient {
    pub fn new(transport: Arc<dyn SessionTransport>) -> Self {
        Self::with_event_buffer(transport, DEFAULT_EVENT_BUFFER)
    }

    /// Creates a client with a bounded per-subscriber event buffer.
    pub fn with_event_buffer(transport: Arc<dyn SessionTransport>, event_buffer: usize) -> Self {
        assert!(event_buffer > 0, "event buffer must be non-zero");
        let inner = Arc::new(ClientInner {
            transport,
            pending: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(HashMap::new()),
            cursors: Mutex::new(HashMap::new()),
            reader: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            next_subscriber_id: AtomicU64::new(1),
            event_buffer,
            disposed: AtomicBool::new(false),
            terminated: AtomicBool::new(false),
            terminated_notify: Notify::new(),
        });
        let reader_inner = Arc::clone(&inner);
        let reader = tokio::spawn(async move { reader_loop(reader_inner).await });
        *inner.reader.lock().unwrap_or_else(|lock| lock.into_inner()) = Some(reader);
        Self { inner }
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::Acquire)
    }

    /// Sends a validated request and awaits its correlated response or protocol error.
    pub async fn request(&self, request: RequestEnvelope) -> Result<Value, ClientError> {
        request.validate()?;
        if self.is_disposed() {
            return Err(ClientError::Disposed);
        }

        let id = request.id.clone();
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            if self.is_disposed() {
                return Err(ClientError::Disposed);
            }
            if pending.contains_key(&id) {
                return Err(ClientError::DuplicateRequestId(id));
            }
            pending.insert(id.clone(), sender);
        }

        if let Err(error) = self.inner.transport.send(Envelope::Request(request)).await {
            let error = ClientError::Transport(error);
            self.inner.terminate(error.clone());
            self.stop_reader().await;
            return Err(error);
        }

        receiver.await.unwrap_or(Err(ClientError::Disposed))
    }

    /// Builds a request from explicit caller metadata and awaits its result.
    pub async fn call(
        &self,
        id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
        meta: RequestMeta,
    ) -> Result<Value, ClientError> {
        let request = RequestEnvelope::new(id, method, params, meta)?;
        self.request(request).await
    }

    /// Requests replay from an explicit cursor. The client generates only the correlation ID;
    /// idempotency metadata remains caller supplied.
    pub async fn replay(
        &self,
        session_id: impl Into<String>,
        cursor: Option<String>,
        limit: Option<u64>,
        meta: RequestMeta,
    ) -> Result<Value, ClientError> {
        let request =
            RequestEnvelope::replay(self.next_id("replay"), session_id, cursor, limit, meta)?;
        self.request(request).await
    }

    /// Replays a session after the last event cursor observed by this client.
    pub async fn reconnect(
        &self,
        session_id: impl Into<String>,
        limit: Option<u64>,
        meta: RequestMeta,
    ) -> Result<Value, ClientError> {
        let session_id = session_id.into();
        let cursor = self.last_cursor(&session_id);
        self.replay(session_id, cursor, limit, meta).await
    }

    /// Sends the canonical mutating cancellation request with caller-owned idempotency metadata.
    pub async fn cancel(
        &self,
        request_id: impl Into<String>,
        meta: RequestMeta,
    ) -> Result<Value, ClientError> {
        let request_id = request_id.into();
        let request = RequestEnvelope::new_mutating(
            self.next_id("cancel"),
            CANCEL_METHOD,
            json!({ "requestId": request_id }),
            meta,
        )?;
        self.request(request).await
    }

    pub fn last_cursor(&self, session_id: &str) -> Option<String> {
        self.inner
            .cursors
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .get(session_id)
            .cloned()
    }

    /// Subscribes to ordered events received after this call.
    pub fn events(&self) -> EventStream {
        let (events_sender, events_receiver) = mpsc::channel(self.inner.event_buffer);
        let (terminal_sender, terminal_receiver) = oneshot::channel();
        let id = self
            .inner
            .next_subscriber_id
            .fetch_add(1, Ordering::Relaxed);

        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        if self.is_disposed() {
            drop(subscribers);
            let _ = terminal_sender.send(ClientError::Disposed);
        } else {
            subscribers.insert(
                id,
                EventSubscriber {
                    events: events_sender,
                    terminal: Some(terminal_sender),
                },
            );
        }

        EventStream {
            id,
            owner: Arc::downgrade(&self.inner),
            events: ReceiverStream::new(events_receiver),
            terminal: terminal_receiver,
            terminated: false,
        }
    }

    /// Stops the reader and deterministically fails pending requests and event streams.
    pub async fn dispose(&self) {
        let terminated = self.inner.terminated_notify.notified();
        if self.inner.disposed.swap(true, Ordering::AcqRel) {
            if !self.inner.terminated.load(Ordering::Acquire) {
                terminated.await;
            }
            return;
        }
        self.stop_reader().await;
        self.inner.terminate(ClientError::Disposed);
    }

    async fn stop_reader(&self) {
        let reader = self
            .inner
            .reader
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .take();
        if let Some(reader) = reader {
            reader.abort();
            let _ = reader.await;
        }
    }

    fn next_id(&self, operation: &str) -> String {
        let sequence = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        format!("sdk-{operation}-{sequence}")
    }
}

/// Ordered event stream associated with one client subscription.
pub struct EventStream {
    id: u64,
    owner: Weak<ClientInner>,
    events: ReceiverStream<Result<EventEnvelope, ClientError>>,
    terminal: oneshot::Receiver<ClientError>,
    terminated: bool,
}

impl Stream for EventStream {
    type Item = Result<EventEnvelope, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.events).poll_next(context) {
            Poll::Ready(Some(event)) => return Poll::Ready(Some(event)),
            Poll::Pending => {}
            Poll::Ready(None) => {}
        }
        if let Poll::Ready(result) = Pin::new(&mut self.terminal).poll(context) {
            self.terminated = true;
            return match result {
                Ok(error) => Poll::Ready(Some(Err(error))),
                Err(_) => Poll::Ready(None),
            };
        }
        Poll::Pending
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner
                .subscribers
                .lock()
                .unwrap_or_else(|lock| lock.into_inner())
                .remove(&self.id);
        }
    }
}

async fn reader_loop(inner: Arc<ClientInner>) {
    loop {
        let envelope = match inner.transport.recv().await {
            Ok(envelope) => envelope,
            Err(error) => {
                inner.terminate(ClientError::Transport(error));
                return;
            }
        };

        match envelope {
            Envelope::Response(response) => {
                if !inner.complete(&response.id, Ok(response.result)) {
                    inner.terminate(ClientError::UnexpectedEnvelope("response"));
                    return;
                }
            }
            Envelope::Error(error) => match error.id.clone() {
                Some(id) => {
                    if !inner.complete(&id, Err(ClientError::Protocol(error))) {
                        inner.terminate(ClientError::UnexpectedEnvelope("error"));
                        return;
                    }
                }
                None => {
                    inner.terminate(ClientError::Protocol(error));
                    return;
                }
            },
            Envelope::Event(event) => inner.publish(event),
            Envelope::Request(_) => {
                inner.terminate(ClientError::UnexpectedEnvelope("request"));
                return;
            }
        }
    }
}
