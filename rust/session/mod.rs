mod event;
pub(crate) mod outbox;
pub(crate) mod store;

use std::path::Path;

pub(crate) use event::{SessionEventV2, SessionPayloadV2, SESSION_EVENT_SCHEMA_VERSION};
pub(crate) use outbox::OutboxConsumer;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrainReport {
    pub(crate) delivered: usize,
    pub(crate) retried: usize,
    pub(crate) pending: usize,
}

#[allow(dead_code)]
/// Consumer adapter boundary. Implementations MUST commit `idempotency_key`
/// in the same transaction as their local side effect (or pass it to the
/// remote idempotency-key field) before returning success.
pub(crate) trait SessionOutboxConsumer {
    fn consumer(&self) -> OutboxConsumer;
    fn deliver(&mut self, event: &SessionEventV2, idempotency_key: &str) -> Result<(), String>;
}

#[allow(dead_code)]
pub(crate) fn drain_outbox<C: SessionOutboxConsumer>(
    dir: &Path,
    consumer: &mut C,
    now_epoch_seconds: u64,
    limit: usize,
) -> Result<DrainReport, String> {
    const LEASE_SECONDS: u64 = 30;
    const MAX_ATTEMPTS: u32 = 12;
    let mut delivered = 0;
    let mut retried = 0;
    for _ in 0..limit {
        let ledger = store::read_events(dir)?;
        let Some(item) = outbox::claim(
            dir,
            &ledger.events,
            consumer.consumer(),
            now_epoch_seconds,
            LEASE_SECONDS,
            MAX_ATTEMPTS,
        )?
        else {
            break;
        };
        let event = ledger
            .events
            .iter()
            .find(|event| event.event_id == item.event_id)
            .ok_or_else(|| format!("outbox event disappeared: {}", item.event_id))?;
        match consumer.deliver(event, &item.idempotency_key) {
            Ok(()) => {
                outbox::complete(dir, &item)?;
                delivered += 1;
            }
            Err(error) => {
                outbox::retry(dir, &item, &error)?;
                retried += 1;
                break;
            }
        }
    }
    let ledger = store::read_events(dir)?;
    let pending = outbox::pending_count(dir, &ledger.events, now_epoch_seconds)?;
    Ok(DrainReport {
        delivered,
        retried,
        pending,
    })
}
