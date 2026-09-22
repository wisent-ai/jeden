//! The asynchronous client for one session: requests out, responses and events
//! back, with the reader task and the failure types beside it.

use super::protocol::{Envelope, RequestEnvelope, RequestMeta};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot, Notify};

const DEFAULT_EVENT_BUFFER: usize = 256;
const CANCEL_METHOD: &str = "request.cancel";

mod errors;
mod inner;
mod stream;

pub use errors::{ClientError, SessionTransport, TransportError};
use inner::{reader_loop, ClientInner, EventSubscriber};
pub use stream::EventStream;
use tokio_stream::wrappers::ReceiverStream;

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
    // `ClientError` is published SDK surface; boxing it would break every
    // downstream `match` compiled against this signature.
    #[allow(clippy::result_large_err)]
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
    // `ClientError` is published SDK surface; boxing it would break every
    // downstream `match` compiled against this signature.
    #[allow(clippy::result_large_err)]
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
    // `ClientError` is published SDK surface; boxing it would break every
    // downstream `match` compiled against this signature.
    #[allow(clippy::result_large_err)]
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
    // `ClientError` is published SDK surface; boxing it would break every
    // downstream `match` compiled against this signature.
    #[allow(clippy::result_large_err)]
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
    // `ClientError` is published SDK surface; boxing it would break every
    // downstream `match` compiled against this signature.
    #[allow(clippy::result_large_err)]
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
