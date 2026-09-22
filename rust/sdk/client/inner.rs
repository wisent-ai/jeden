//! The shared state behind every clone of a session client, and the single
//! task that reads from the transport on their behalf.
//!
//! Split out of `sdk/client.rs`, which had grown past the module line cap.

use super::errors::{ClientError, SessionTransport};
use super::super::protocol::{Envelope, EventEnvelope};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;

pub(super) struct EventSubscriber {
    pub(super) events: mpsc::Sender<Result<EventEnvelope, ClientError>>,
    pub(super) terminal: Option<oneshot::Sender<ClientError>>,
}

pub(super) struct ClientInner {
    pub(super) transport: Arc<dyn SessionTransport>,
    pub(super) pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, ClientError>>>>,
    pub(super) subscribers: Mutex<HashMap<u64, EventSubscriber>>,
    pub(super) cursors: Mutex<HashMap<String, String>>,
    pub(super) reader: Mutex<Option<JoinHandle<()>>>,
    pub(super) next_request_id: AtomicU64,
    pub(super) next_subscriber_id: AtomicU64,
    pub(super) event_buffer: usize,
    pub(super) disposed: AtomicBool,
    pub(super) terminated: AtomicBool,
    pub(super) terminated_notify: Notify,
}

impl ClientInner {
    pub(super) fn terminate(&self, error: ClientError) {
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

pub(super) async fn reader_loop(inner: Arc<ClientInner>) {
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
