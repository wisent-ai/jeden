//! What each session event owes its consumers, recorded with the event.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutboxConsumer {
    Memory,
    Collaboration,
    Telemetry,
    RemoteReplication,
}

impl OutboxConsumer {
    pub(crate) const ALL: [Self; 4] = [
        Self::Memory,
        Self::Collaboration,
        Self::Telemetry,
        Self::RemoteReplication,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutboxItem {
    pub(crate) consumer: OutboxConsumer,
    pub(crate) event_id: String,
    pub(crate) idempotency_key: String,
    pub(crate) attempt: u32,
    pub(crate) lease_until: u64,
    pub(crate) state: OutboxState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutboxState {
    Pending,
    Leased,
    Delivered,
    DeadLetter,
}

impl OutboxItem {
    pub(crate) fn pending(consumer: OutboxConsumer, event_id: &str) -> Self {
        let name = serde_json::to_value(consumer)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        Self {
            consumer,
            event_id: event_id.to_owned(),
            idempotency_key: format!("session-event:{event_id}:{name}"),
            attempt: 0,
            lease_until: 0,
            state: OutboxState::Pending,
        }
    }
}
