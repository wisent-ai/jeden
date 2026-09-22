//! The confirmation step between choosing something destructive and running
//! it.
//!
//! Split out of `tui/view/mod.rs`, which had grown past the module line cap.

use crate::cli::i18n::tr;
use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmState {
    pub label: String,
    pub detail: String,
    pub command: String,
    pub confirmed: bool,
    /// Chrome language inherited from the picker that raised the confirmation.
    pub lang: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmEvent {
    Pending,
    Cancelled,
    Submit(String),
}

impl ConfirmState {
    pub fn new(label: String, detail: String, command: String, lang: String) -> Self {
        Self {
            label,
            detail,
            command,
            confirmed: false,
            lang,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConfirmEvent {
        match key.code {
            KeyCode::Esc => ConfirmEvent::Cancelled,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                self.confirmed = !self.confirmed;
                ConfirmEvent::Pending
            }
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
                if self.confirmed {
                    ConfirmEvent::Submit(self.command.clone())
                } else {
                    ConfirmEvent::Cancelled
                }
            }
            _ => ConfirmEvent::Pending,
        }
    }
}
