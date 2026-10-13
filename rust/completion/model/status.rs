use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    VerificationRequested,
    Done,
    Blocked,
    Paused,
    Cancelled,
}

impl TaskStatus {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// The status reads in text as it does on the wire: serde writes the renamed
/// variant straight into the formatter, so there is no second spelling to
/// drift and no conversion that could fail and need an invented word.
impl std::fmt::Display for TaskStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(formatter)
    }
}
