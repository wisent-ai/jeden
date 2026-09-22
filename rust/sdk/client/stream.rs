//! The ordered event stream a caller holds for one subscription.
//!
//! Split out of `sdk/client.rs`, which had grown past the module line cap.

use super::errors::ClientError;
use super::inner::ClientInner;
use super::super::protocol::EventEnvelope;
use std::future::Future;
use std::pin::Pin;
use std::sync::Weak;
use std::task::{Context, Poll};
use tokio::sync::oneshot;
use tokio_stream::{wrappers::ReceiverStream, Stream};

/// Ordered event stream associated with one client subscription.
pub struct EventStream {
    pub(super) id: u64,
    pub(super) owner: Weak<ClientInner>,
    pub(super) events: ReceiverStream<Result<EventEnvelope, ClientError>>,
    pub(super) terminal: oneshot::Receiver<ClientError>,
    pub(super) terminated: bool,
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
