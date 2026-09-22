use std::cmp::Ordering;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const MAX_UNDO_STEPS: usize = 64;
const MAX_HISTORY_ITEMS: usize = 100;
const MAX_BUFFER_BYTES: usize = 1024 * 1024;

pub const EDITOR_KEYMAP_NAMESPACE: &str = "editor";
pub const EXTERNAL_EDITOR_ACTION_ID: &str = "editor.external";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorLimitError {
    pub limit_bytes: usize,
}

impl std::fmt::Display for EditorLimitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Editor input limit exceeded ({} bytes)",
            self.limit_bytes
        )
    }
}

impl std::error::Error for EditorLimitError {}

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

#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct EditorState {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    preferred_column: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<Snapshot>,
    keymap: ActionKeyMap,
    last_error: Option<EditorLimitError>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new(ActionKeyMap::default())
    }
}

impl EditorState {
    pub fn new(keymap: ActionKeyMap) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: None,
            preferred_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            last_error: None,
            keymap,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn take_error(&mut self) -> Option<EditorLimitError> {
        self.last_error.take()
    }
    pub fn action_for(&self, event: KeyEvent) -> Option<EditorAction> {
        self.keymap.action_for(event)
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.len() > MAX_BUFFER_BYTES {
            self.last_error = Some(EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            });
            return;
        }
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
    }

    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
        self.history_draft = None;
        self.undo.clear();
        self.redo.clear();
        text
    }

    pub fn clear(&mut self) {
        if !self.text.is_empty() {
            self.record_undo();
            self.text.clear();
            self.cursor = 0;
            self.anchor = None;
            self.preferred_column = None;
        }
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            None
        } else {
            Some(ordered(anchor, self.cursor))
        }
    }

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

    pub fn insert(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        if !self.can_replace_selection_with(value.len()) {
            self.last_error = Some(EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            });
            return;
        }
        self.record_undo();
        self.replace_selection(value);
    }

    pub fn paste(&mut self, value: &str) {
        if !self.can_replace_selection_with(value.len()) {
            self.last_error = Some(EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            });
            return;
        }
        let normalized = normalize_paste(value);
        if normalized.is_empty() {
            return;
        }
        self.record_undo();
        self.replace_selection(&normalized);
    }

    pub fn delete_backward(&mut self) {
        if self.selection().is_some() {
            self.record_undo();
            self.replace_selection("");
            return;
        }
        let start = previous_boundary(&self.text, self.cursor);
        if start != self.cursor {
            self.record_undo();
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.preferred_column = None;
        }
    }

    pub fn delete_forward(&mut self) {
        if self.selection().is_some() {
            self.record_undo();
            self.replace_selection("");
            return;
        }
        let end = next_boundary(&self.text, self.cursor);
        if end != self.cursor {
            self.record_undo();
            self.text.replace_range(self.cursor..end, "");
            self.preferred_column = None;
        }
    }

    pub fn push_history(&mut self, value: String) {
        if value.is_empty() || self.history.last() == Some(&value) {
            return;
        }
        if self.history.len() == MAX_HISTORY_ITEMS {
            self.history.remove(0);
        }
        self.history.push(value);
        self.history_index = None;
        self.history_draft = None;
    }

    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.history_draft = Some(self.snapshot());
        }
        let index = self
            .history_index
            .map_or(self.history.len() - 1, |i| i.saturating_sub(1));
        self.load_history(index);
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.load_history(index + 1);
        } else if let Some(draft) = self.history_draft.take() {
            self.restore(draft);
            self.history_index = None;
        }
    }

    pub fn replace_all_transaction(&mut self, text: String) -> Result<bool, EditorLimitError> {
        if text.len() > MAX_BUFFER_BYTES {
            let error = EditorLimitError {
                limit_bytes: MAX_BUFFER_BYTES,
            };
            self.last_error = Some(error);
            return Err(error);
        }
        if text == self.text {
            return Ok(false);
        }
        self.record_undo();
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
        Ok(true)
    }

    fn can_replace_selection_with(&self, bytes: usize) -> bool {
        let removed = self.selection().map_or(0, |(start, end)| end - start);
        self.text
            .len()
            .saturating_sub(removed)
            .saturating_add(bytes)
            <= MAX_BUFFER_BYTES
    }

    fn replace_selection(&mut self, value: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.text.replace_range(start..end, value);
        self.cursor = start + value.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = None;
    }

    fn move_to(&mut self, cursor: usize, selecting: bool) {
        if selecting {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = cursor;
        self.preferred_column = None;
    }

    fn move_vertical(&mut self, direction: isize) {
        let start = line_start(&self.text, self.cursor);
        let column = self
            .preferred_column
            .unwrap_or_else(|| UnicodeWidthStr::width(&self.text[start..self.cursor]));
        let target_start = if direction < 0 {
            if start == 0 {
                return;
            }
            line_start(&self.text, start.saturating_sub(1))
        } else {
            let end = line_end(&self.text, self.cursor);
            if end == self.text.len() {
                return;
            }
            end + 1
        };
        let target_end = line_end(&self.text, target_start);
        self.cursor = byte_at_display_column(&self.text, target_start, target_end, column);
        self.anchor = None;
        self.preferred_column = Some(column);
    }

    fn record_undo(&mut self) {
        if self.undo.len() == MAX_UNDO_STEPS {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo.pop() else {
            return;
        };
        if self.redo.len() == MAX_UNDO_STEPS {
            self.redo.remove(0);
        }
        self.redo.push(self.snapshot());
        self.restore(snapshot);
    }

    fn redo(&mut self) {
        let Some(snapshot) = self.redo.pop() else {
            return;
        };
        if self.undo.len() == MAX_UNDO_STEPS {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.restore(snapshot);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.anchor = snapshot.anchor;
        self.preferred_column = None;
    }

    fn load_history(&mut self, index: usize) {
        self.text.clone_from(&self.history[index]);
        self.cursor = self.text.len();
        self.anchor = None;
        self.preferred_column = None;
        self.history_index = Some(index);
    }
}

fn ordered(a: usize, b: usize) -> (usize, usize) {
    match a.cmp(&b) {
        Ordering::Less | Ordering::Equal => (a, b),
        Ordering::Greater => (b, a),
    }
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .grapheme_indices(true)
        .nth(1)
        .map_or(text.len(), |(index, _)| cursor + index)
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index)
}

fn word_left(text: &str, cursor: usize) -> usize {
    let before = &text[..cursor];
    let mut target = 0;
    let mut seen_word = false;
    for (index, grapheme) in before.grapheme_indices(true).rev() {
        let word = grapheme.chars().any(char::is_alphanumeric) || grapheme == "_";
        if word {
            seen_word = true;
            target = index;
        } else if seen_word {
            break;
        } else {
            target = index;
        }
    }
    target
}

fn word_right(text: &str, cursor: usize) -> usize {
    let mut seen_word = false;
    for (offset, grapheme) in text[cursor..].grapheme_indices(true) {
        let word = grapheme.chars().any(char::is_alphanumeric) || grapheme == "_";
        if word {
            seen_word = true;
        } else if seen_word {
            return cursor + offset;
        }
    }
    text.len()
}

fn byte_at_display_column(text: &str, start: usize, end: usize, target: usize) -> usize {
    let mut width = 0;
    let mut byte = start;
    for (offset, grapheme) in text[start..end].grapheme_indices(true) {
        let next = width + UnicodeWidthStr::width(grapheme);
        if next > target {
            break;
        }
        width = next;
        byte = start + offset + grapheme.len();
    }
    byte
}

fn normalize_paste(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\n');
            }
            '\n' | '\t' => normalized.push(ch),
            ch if !ch.is_control() && !matches!(ch as u32, 0x80..=0x9f) => normalized.push(ch),
            _ => {}
        }
    }
    normalized
}
