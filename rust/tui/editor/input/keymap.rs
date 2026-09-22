//! Which key does what in the prompt editor, and what happens when one is
//! pressed.
//!
//! Split out of `tui/editor/mod.rs`, which had grown past the module line cap.

use super::super::EditorState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::tui::editor::EXTERNAL_EDITOR_ACTION_ID;
use crate::tui::editor::input::cursor::line_end;
use crate::tui::editor::input::cursor::line_start;
use crate::tui::editor::input::cursor::next_boundary;
use crate::tui::editor::input::cursor::previous_boundary;
use crate::tui::editor::input::cursor::word_left;
use crate::tui::editor::input::cursor::word_right;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    MoveLeft,
    MoveRight,
    MoveWordLeft,
    MoveWordRight,
    MoveLineStart,
    MoveLineEnd,
    MoveBufferStart,
    MoveBufferEnd,
    MoveUp,
    MoveDown,
    SelectLeft,
    SelectRight,
    SelectWordLeft,
    SelectWordRight,
    DeleteBackward,
    DeleteForward,
    Undo,
    Redo,
    InsertNewline,
    HistoryPrevious,
    HistoryNext,
    ExternalEditor,
}

impl EditorAction {
    pub(super) const fn action_id(self) -> &'static str {
        match self {
            Self::MoveLeft => "editor.move-left",
            Self::MoveRight => "editor.move-right",
            Self::MoveWordLeft => "editor.move-word-left",
            Self::MoveWordRight => "editor.move-word-right",
            Self::MoveLineStart => "editor.move-line-start",
            Self::MoveLineEnd => "editor.move-line-end",
            Self::MoveBufferStart => "editor.move-buffer-start",
            Self::MoveBufferEnd => "editor.move-buffer-end",
            Self::MoveUp => "editor.move-up",
            Self::MoveDown => "editor.move-down",
            Self::SelectLeft => "editor.select-left",
            Self::SelectRight => "editor.select-right",
            Self::SelectWordLeft => "editor.select-word-left",
            Self::SelectWordRight => "editor.select-word-right",
            Self::DeleteBackward => "editor.delete-backward",
            Self::DeleteForward => "editor.delete-forward",
            Self::Undo => "editor.undo",
            Self::Redo => "editor.redo",
            Self::InsertNewline => "editor.insert-newline",
            Self::HistoryPrevious => "editor.history-previous",
            Self::HistoryNext => "editor.history-next",
            Self::ExternalEditor => EXTERNAL_EDITOR_ACTION_ID,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub action: EditorAction,
}

#[derive(Debug, Clone)]
pub struct ActionKeyMap {
    bindings: Vec<KeyBinding>,
}

impl Default for ActionKeyMap {
    fn default() -> Self {
        use EditorAction::*;
        let bindings = vec![
            bind(KeyCode::Left, KeyModifiers::NONE, MoveLeft),
            bind(KeyCode::Right, KeyModifiers::NONE, MoveRight),
            bind(KeyCode::Left, KeyModifiers::CONTROL, MoveWordLeft),
            bind(KeyCode::Right, KeyModifiers::CONTROL, MoveWordRight),
            bind(KeyCode::Char('b'), KeyModifiers::ALT, MoveWordLeft),
            bind(KeyCode::Char('f'), KeyModifiers::ALT, MoveWordRight),
            bind(KeyCode::Home, KeyModifiers::NONE, MoveLineStart),
            bind(KeyCode::End, KeyModifiers::NONE, MoveLineEnd),
            bind(KeyCode::Home, KeyModifiers::CONTROL, MoveBufferStart),
            bind(KeyCode::End, KeyModifiers::CONTROL, MoveBufferEnd),
            bind(KeyCode::Up, KeyModifiers::NONE, MoveUp),
            bind(KeyCode::Down, KeyModifiers::NONE, MoveDown),
            bind(KeyCode::Left, KeyModifiers::SHIFT, SelectLeft),
            bind(KeyCode::Right, KeyModifiers::SHIFT, SelectRight),
            bind(
                KeyCode::Left,
                KeyModifiers::SHIFT | KeyModifiers::CONTROL,
                SelectWordLeft,
            ),
            bind(
                KeyCode::Right,
                KeyModifiers::SHIFT | KeyModifiers::CONTROL,
                SelectWordRight,
            ),
            bind(KeyCode::Backspace, KeyModifiers::NONE, DeleteBackward),
            bind(KeyCode::Delete, KeyModifiers::NONE, DeleteForward),
            bind(KeyCode::Char('z'), KeyModifiers::CONTROL, Undo),
            bind(KeyCode::Char('y'), KeyModifiers::CONTROL, Redo),
            bind(
                KeyCode::Char('z'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                Redo,
            ),
            bind(KeyCode::Char('e'), KeyModifiers::ALT, ExternalEditor),
        ];
        Self { bindings }
    }
}

fn bind(code: KeyCode, modifiers: KeyModifiers, action: EditorAction) -> KeyBinding {
    KeyBinding {
        code,
        modifiers,
        action,
    }
}

impl ActionKeyMap {
    pub(super) fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    pub fn action_for(&self, event: KeyEvent) -> Option<EditorAction> {
        let modifiers = event.modifiers
            & (KeyModifiers::SHIFT
                | KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER);
        self.bindings
            .iter()
            .find(|binding| binding.code == event.code && binding.modifiers == modifiers)
            .map(|binding| binding.action)
    }
}

impl EditorState {
    pub fn handle_key(&mut self, event: KeyEvent) -> bool {
        if let Some(action) = self.keymap.action_for(event) {
            if action == EditorAction::ExternalEditor {
                return false;
            }
            self.apply(action);
            return true;
        }
        if let KeyCode::Char(ch) = event.code {
            if !event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            {
                self.insert(&ch.to_string());
                return true;
            }
        }
        false
    }

    pub fn apply(&mut self, action: EditorAction) {
        use EditorAction::*;
        match action {
            MoveLeft => {
                let target = self
                    .selection()
                    .map(|(start, _)| start)
                    .unwrap_or_else(|| previous_boundary(&self.text, self.cursor));
                self.move_to(target, false);
            }
            MoveRight => {
                let target = self
                    .selection()
                    .map(|(_, end)| end)
                    .unwrap_or_else(|| next_boundary(&self.text, self.cursor));
                self.move_to(target, false);
            }
            MoveWordLeft => {
                let target = self
                    .selection()
                    .map(|(start, _)| start)
                    .unwrap_or_else(|| word_left(&self.text, self.cursor));
                self.move_to(target, false);
            }
            MoveWordRight => {
                let target = self
                    .selection()
                    .map(|(_, end)| end)
                    .unwrap_or_else(|| word_right(&self.text, self.cursor));
                self.move_to(target, false);
            }
            MoveLineStart => self.move_to(line_start(&self.text, self.cursor), false),
            MoveLineEnd => self.move_to(line_end(&self.text, self.cursor), false),
            MoveBufferStart => self.move_to(0, false),
            MoveBufferEnd => self.move_to(self.text.len(), false),
            MoveUp => self.move_vertical(-1),
            MoveDown => self.move_vertical(1),
            SelectLeft => self.move_to(previous_boundary(&self.text, self.cursor), true),
            SelectRight => self.move_to(next_boundary(&self.text, self.cursor), true),
            SelectWordLeft => self.move_to(word_left(&self.text, self.cursor), true),
            SelectWordRight => self.move_to(word_right(&self.text, self.cursor), true),
            DeleteBackward => self.delete_backward(),
            DeleteForward => self.delete_forward(),
            Undo => self.undo(),
            Redo => self.redo(),
            InsertNewline => self.insert("\n"),
            HistoryPrevious => self.history_previous(),
            HistoryNext => self.history_next(),
            ExternalEditor => {}
        }
    }

}
