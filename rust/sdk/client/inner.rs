//! The shared state behind every clone of a session client, and the single
//! task that reads from the transport on their behalf.
//!
//! Split out of `sdk/client.rs`, which had grown past the module line cap.

use super::super::protocol::{Envelope, EventEnvelope};
use super::errors::{ClientError, SessionTransport};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

// The client's maps change by single inserts, removes and takes, so a lock a
// panicking holder poisoned is entered as it stands rather than taken for a
// failed client.
use crate::panicked::held;

pub(super) struct EventSubscriber {
    pub(super) events: mpsc::UnboundedSender<Result<EventEnvelope, ClientError>>,
    pub(super) terminal: Option<oneshot::Sender<ClientError>>,
}

pub(super) struct ClientInner {
    pub(super) transport: Arc<dyn SessionTransport>,
    pub(super) pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, ClientError>>>>,
    /// Every live event stream, by the identity it was given when it subscribed.
    pub(super) subscribers: Mutex<HashMap<Uuid, EventSubscriber>>,
    pub(super) cursors: Mutex<HashMap<String, String>>,
    pub(super) reader: Mutex<Option<JoinHandle<()>>>,
    pub(super) disposed: AtomicBool,
    pub(super) terminated: AtomicBool,
    pub(super) terminated_notify: Notify,
}

impl ClientInner {
    pub(super) fn terminate(&self, error: ClientError) {
        self.disposed.store(true, Ordering::Release);

        let pending = {
            let mut pending = held(&self.pending);
            std::mem::take(&mut *pending)
        };
        for sender in pending.into_values() {
            // A waiter that already gave up needs no answer.
            let _unreceived = sender.send(Err(error.clone()));
        }

        let subscribers = {
            let mut subscribers = held(&self.subscribers);
            std::mem::take(&mut *subscribers)
        };
        for mut subscriber in subscribers.into_values() {
            if let Some(sender) = subscriber.terminal.take() {
                let _unreceived = sender.send(error.clone());
            }
        }
        self.terminated.store(true, Ordering::Release);
        self.terminated_notify.notify_waiters();
    }

    fn complete(&self, id: &str, result: Result<Value, ClientError>) -> bool {
        let sender = held(&self.pending).remove(id);
        match sender {
            Some(sender) => {
                let _unreceived = sender.send(result);
                true
            }
            None => false,
        }
    }

    fn publish(&self, event: EventEnvelope) {
        held(&self.cursors).insert(event.session_id.clone(), event.cursor.clone());

        let mut subscribers = held(&self.subscribers);
        // Every event reaches every subscriber, however far behind it reads;
        // only a subscriber whose stream is gone is dropped.
        subscribers.retain(|_, subscriber| subscriber.events.send(Ok(event.clone())).is_ok());
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
