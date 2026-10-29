//! What each session event owes its consumers, recorded with the event.

use serde::{Deserialize, Serialize};

/// A consumer travels on the wire and in idempotency keys as [`Self::name`];
/// serde reads and writes that same name, so the two cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub(crate) enum OutboxConsumer {
    Memory,
    Collaboration,
    Telemetry,
    RemoteReplication,
}

impl OutboxConsumer {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Memory,
        Self::Collaboration,
        Self::Telemetry,
        Self::RemoteReplication,
    ];

    /// The consumer's wire name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Collaboration => "collaboration",
            Self::Telemetry => "telemetry",
            Self::RemoteReplication => "remote_replication",
        }
    }
}

impl From<OutboxConsumer> for &'static str {
    fn from(consumer: OutboxConsumer) -> Self {
        consumer.name()
    }
}

impl TryFrom<String> for OutboxConsumer {
    type Error = String;

    /// The consumer `name` names; an unknown name is refused naming it.
    fn try_from(name: String) -> Result<Self, Self::Error> {
        match OutboxConsumer::ALL
            .iter()
            .copied()
            .find(|consumer| consumer.name() == name)
        {
            Some(consumer) => Ok(consumer),
            None => Err(format!("{name} is not an outbox consumer")),
        }
    }
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
    /// What `consumer` is owed for `event_id` when the event is recorded: no
    /// attempt made and no lease held, keyed by the consumer's wire name.
    pub(crate) fn pending(consumer: OutboxConsumer, event_id: &str) -> Self {
        let name = consumer.name();
        Self {
            consumer,
            event_id: event_id.to_owned(),
            idempotency_key: format!("session-event:{event_id}:{name}"),
            attempt: crate::tally::ORIGIN as u32,
            lease_until: crate::tally::ORIGIN as u64,
            state: OutboxState::Pending,
        }
    }
}
