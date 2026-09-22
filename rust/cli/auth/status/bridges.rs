//! The two ways a login conversation reaches a person: the terminal the
//! operator is already looking at, and a background turn.
//!
//! Split out of `cli/auth/status.rs`, which had grown past the module line
//! cap.

use std::io::{self, Write};

use crate::control_plane::weles::{InteractionBridge, OperationEvent};

pub(super) struct ConsoleBridge;
impl InteractionBridge for ConsoleBridge {
    fn elicit(&self, prompt: &str, options: &[String], _secret: bool) -> Result<String, String> {
        eprint!("{prompt}");
        if !options.is_empty() {
            eprint!(" [{}]", options.join("/"));
        }
        eprint!(": ");
        io::stderr().flush().map_err(|error| error.to_string())?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        let answer = answer.trim().to_string();
        if answer.is_empty() {
            Err("authentication input cannot be empty".into())
        } else {
            Ok(answer)
        }
    }
    fn event(&self, event: &OperationEvent) {
        match event {
            OperationEvent::Status { message } => eprintln!("Weles: {message}"),
            OperationEvent::DeviceCode {
                verification_uri,
                user_code,
                ..
            } => {
                eprintln!("Open {verification_uri} and enter code {user_code}");
                if let Some(qr) = crate::qr::render(verification_uri) {
                    eprintln!("{qr}");
                }
            }
            _ => {}
        }
    }
}

/// Bridge for TUI background turns: status updates go to the skeleton's note,
/// the device-code block (with QR) streams into the live text region, and
/// questions use the live prompt — so `/login <provider>` stays cancellable
/// with Esc and never touches stderr. Falls back to an error when the turn
/// cannot ask questions (non-interactive contexts).
pub(crate) struct TurnBridge<'a> {
    pub progress: &'a dyn Fn(&str),
    pub stream: &'a dyn Fn(&str),
    pub ask_user: Option<crate::tool_runtime::AskUserFn<'a>>,
}

impl InteractionBridge for TurnBridge<'_> {
    fn elicit(&self, prompt: &str, options: &[String], _secret: bool) -> Result<String, String> {
        match self.ask_user {
            Some(ask) => ask(prompt, options),
            None => Err("interactive input is unavailable during this turn".into()),
        }
    }
    fn event(&self, event: &OperationEvent) {
        match event {
            OperationEvent::Status { message } => (self.progress)(message),
            OperationEvent::DeviceCode {
                verification_uri,
                user_code,
                ..
            } => {
                (self.progress)(&format!("enter code {user_code} at {verification_uri}"));
                let qr = crate::qr::render(verification_uri)
                    .map(|qr| format!("\n{qr}"))
                    .unwrap_or_default();
                (self.stream)(&format!(
                    "Open {verification_uri} and enter code {user_code}.{qr}\n"
                ));
            }
            _ => {}
        }
    }
}
