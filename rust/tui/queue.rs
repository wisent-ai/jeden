use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const MAX_QUEUED_MESSAGES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAction {
    FollowUp,
    Steer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub action: DeliveryAction,
}

#[derive(Debug, Clone)]
pub struct DeliveryKeyMap {
    bindings: Vec<DeliveryBinding>,
}

impl Default for DeliveryKeyMap {
    fn default() -> Self {
        Self {
            bindings: vec![
                DeliveryBinding {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                    action: DeliveryAction::FollowUp,
                },
                DeliveryBinding {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::CONTROL,
                    action: DeliveryAction::Steer,
                },
            ],
        }
    }
}

impl DeliveryKeyMap {
    pub fn action_for(&self, key: KeyEvent) -> Option<DeliveryAction> {
        let modifiers = key.modifiers
            & (KeyModifiers::SHIFT
                | KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER);
        self.bindings
            .iter()
            .find(|binding| binding.code == key.code && binding.modifiers == modifiers)
            .map(|binding| binding.action)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
    pub id: u64,
    pub text: String,
    pub action: DeliveryAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    Empty,
    Capacity { limit: usize },
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Queued message cannot be empty"),
            Self::Capacity { limit } => {
                write!(formatter, "Follow-up queue limit reached ({limit})")
            }
        }
    }
}

impl std::error::Error for QueueError {}

#[derive(Debug, Clone)]
pub struct FollowUpQueue {
    messages: VecDeque<QueuedMessage>,
    next_id: u64,
    keymap: DeliveryKeyMap,
}

impl Default for FollowUpQueue {
    fn default() -> Self {
        Self {
            messages: VecDeque::new(),
            next_id: 0,
            keymap: DeliveryKeyMap::default(),
        }
    }
}

impl FollowUpQueue {
    pub fn len(&self) -> usize {
        self.messages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
    pub fn action_for(&self, key: KeyEvent) -> Option<DeliveryAction> {
        self.keymap.action_for(key)
    }

    pub fn push(&mut self, text: String, action: DeliveryAction) -> Result<u64, QueueError> {
        if text.trim().is_empty() {
            return Err(QueueError::Empty);
        }
        if self.messages.len() >= MAX_QUEUED_MESSAGES {
            return Err(QueueError::Capacity {
                limit: MAX_QUEUED_MESSAGES,
            });
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.messages.push_back(QueuedMessage { id, text, action });
        Ok(id)
    }

    pub fn pop_next(&mut self) -> Option<QueuedMessage> {
        let steering = self
            .messages
            .iter()
            .position(|message| message.action == DeliveryAction::Steer);
        steering
            .and_then(|index| self.messages.remove(index))
            .or_else(|| self.messages.pop_front())
    }

    pub fn recall_last(&mut self) -> Option<QueuedMessage> {
        self.messages.pop_back()
    }
}
