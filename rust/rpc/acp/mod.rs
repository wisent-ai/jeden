mod agent;
mod mapping;

use agent_client_protocol::{ConnectTo, Lines};
use futures::channel::mpsc;
use futures::executor::block_on;
use serde_json::json;
use std::io::{self, BufRead, Write};
use std::thread;

pub fn serve_stdio() -> Result<(), String> {
    let (incoming_tx, incoming) = mpsc::unbounded::<io::Result<String>>();
    thread::spawn(move || {
        let mut lines = io::BufReader::new(io::stdin()).lines();
        loop {
            match lines.next() {
                Some(Ok(line)) => {
                    if incoming_tx.unbounded_send(Ok(line)).is_err() {
                        return;
                    }
                }
                Some(Err(error)) => {
                    let _ = incoming_tx.unbounded_send(Err(error));
                    return;
                }
                None => std::process::exit(0),
            }
        }
    });
    let outgoing = futures::sink::unfold(
        io::BufWriter::new(io::stdout()),
        |mut stdout, line: String| async move {
            stdout.write_all(line.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            Ok::<_, io::Error>(stdout)
        },
    );
    block_on(agent::build_agent().connect_to(Lines::new(outgoing, incoming)))
        .map_err(|error| error.to_string())
}

pub(crate) fn invalid_params(message: impl ToString) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message.to_string())
}

pub(crate) fn unsupported(
    capability: &str,
    message: impl ToString,
) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(-32004, "Unsupported capability")
        .data(json!({"capability": capability, "detail": message.to_string()}))
}

pub(crate) fn internal(message: impl ToString) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(message.to_string())
}
