//! One terminal event stream shared by the prompt and background turns.

use std::io;

use crossterm::event::{Event, EventStream};
use futures::channel::mpsc::UnboundedReceiver;
use futures::executor::block_on;
use futures::{FutureExt, StreamExt};

#[derive(Default)]
pub(super) struct TerminalInput {
    stream: Option<EventStream>,
}

pub(super) enum InputOrMessage<T> {
    Input(Event),
    Message(Option<T>),
}

impl TerminalInput {
    /// Release the reader before a foreground command takes over stdin.
    pub(super) fn suspend(&mut self) {
        self.stream.take();
    }

    async fn next(&mut self) -> io::Result<Event> {
        self.stream
            .get_or_insert_with(EventStream::new)
            .next()
            .await
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream closed")
            })?
            .map_err(|error| {
                io::Error::new(error.kind(), format!("terminal event read failed: {error}"))
            })
    }

    pub(super) fn read(&mut self) -> io::Result<Event> {
        block_on(self.next())
    }

    /// A key, worker output, or channel closure wakes the renderer.
    pub(super) fn read_or_message<T>(
        &mut self,
        messages: &mut UnboundedReceiver<T>,
    ) -> io::Result<InputOrMessage<T>> {
        block_on(async {
            let input = self.next().fuse();
            let message = messages.next().fuse();
            futures::pin_mut!(input, message);
            futures::select! {
                event = input => event.map(InputOrMessage::Input),
                message = message => Ok(InputOrMessage::Message(message)),
            }
        })
    }
}
