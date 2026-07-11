use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

#[allow(dead_code)]
static OUTBOX_LOCK: Mutex<()> = Mutex::new(());
#[allow(dead_code)]
const OUTBOX_STATE_FILE: &str = "outbox.jsonl";

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

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutboxTransition {
    consumer: OutboxConsumer,
    event_id: String,
    idempotency_key: String,
    attempt: u32,
    lease_until: u64,
    state: OutboxState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[allow(dead_code)]
pub(crate) fn claim(
    dir: &Path,
    events: &[super::event::SessionEventV2],
    consumer: OutboxConsumer,
    now: u64,
    lease_seconds: u64,
    max_attempts: u32,
) -> Result<Option<OutboxItem>, String> {
    let _guard = OUTBOX_LOCK
        .lock()
        .map_err(|_| "session outbox lock poisoned")?;
    let mut effective = effective_items(dir, events)?;
    let next = events
        .iter()
        .flat_map(|event| event.outbox.iter())
        .find(|seed| {
            if seed.consumer != consumer {
                return false;
            }
            let item = effective
                .get(&(seed.event_id.clone(), seed.consumer))
                .unwrap_or(seed);
            item.state == OutboxState::Pending
                || (item.state == OutboxState::Leased && item.lease_until <= now)
        })
        .cloned();
    let Some(mut item) = next else {
        return Ok(None);
    };
    let current = effective
        .remove(&(item.event_id.clone(), item.consumer))
        .unwrap_or_else(|| item.clone());
    if current.attempt >= max_attempts {
        append_transition(
            dir,
            &OutboxTransition {
                consumer,
                event_id: current.event_id,
                idempotency_key: current.idempotency_key,
                attempt: current.attempt,
                lease_until: 0,
                state: OutboxState::DeadLetter,
                error: Some("maximum delivery attempts reached".into()),
            },
        )?;
        return Ok(None);
    }
    item.attempt = current.attempt.saturating_add(1);
    item.lease_until = now.saturating_add(lease_seconds.max(1));
    item.state = OutboxState::Leased;
    append_transition(
        dir,
        &OutboxTransition {
            consumer,
            event_id: item.event_id.clone(),
            idempotency_key: item.idempotency_key.clone(),
            attempt: item.attempt,
            lease_until: item.lease_until,
            state: item.state,
            error: None,
        },
    )?;
    Ok(Some(item))
}

#[allow(dead_code)]
pub(crate) fn complete(dir: &Path, item: &OutboxItem) -> Result<(), String> {
    transition_terminal(dir, item, OutboxState::Delivered, None)
}

#[allow(dead_code)]
pub(crate) fn retry(dir: &Path, item: &OutboxItem, error: &str) -> Result<(), String> {
    transition_terminal(dir, item, OutboxState::Pending, Some(bound_error(error)))
}

#[allow(dead_code)]
fn transition_terminal(
    dir: &Path,
    item: &OutboxItem,
    state: OutboxState,
    error: Option<String>,
) -> Result<(), String> {
    let _guard = OUTBOX_LOCK
        .lock()
        .map_err(|_| "session outbox lock poisoned")?;
    let events = super::store::read_events(dir)?.events;
    let effective = effective_items(dir, &events)?;
    let current = effective
        .get(&(item.event_id.clone(), item.consumer))
        .ok_or_else(|| format!("outbox item not found for {}", item.event_id))?;
    if current.state == OutboxState::Delivered {
        return Ok(());
    }
    if current.state != OutboxState::Leased
        || current.attempt != item.attempt
        || current.lease_until != item.lease_until
    {
        return Err(format!("stale outbox lease for {}", item.event_id));
    }
    append_transition(
        dir,
        &OutboxTransition {
            consumer: item.consumer,
            event_id: item.event_id.clone(),
            idempotency_key: item.idempotency_key.clone(),
            attempt: item.attempt,
            lease_until: 0,
            state,
            error,
        },
    )
}

#[allow(dead_code)]
pub(crate) fn pending_count(
    dir: &Path,
    events: &[super::event::SessionEventV2],
    now: u64,
) -> Result<usize, String> {
    Ok(effective_items(dir, events)?
        .values()
        .filter(|item| {
            item.state == OutboxState::Pending
                || (item.state == OutboxState::Leased && item.lease_until <= now)
        })
        .count())
}

#[allow(dead_code)]
fn effective_items(
    dir: &Path,
    events: &[super::event::SessionEventV2],
) -> Result<HashMap<(String, OutboxConsumer), OutboxItem>, String> {
    let mut items = HashMap::new();
    for item in events.iter().flat_map(|event| event.outbox.iter()) {
        items.insert((item.event_id.clone(), item.consumer), item.clone());
    }
    let path = dir.join(OUTBOX_STATE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(items),
        Err(error) => return Err(error.to_string()),
    };
    if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
        return Err(format!(
            "{} has a truncated transition tail",
            path.display()
        ));
    }
    for (index, line) in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        let transition: OutboxTransition = serde_json::from_slice(line).map_err(|e| {
            format!(
                "{}:{} invalid outbox transition: {}",
                path.display(),
                index + 1,
                e
            )
        })?;
        let key = (transition.event_id.clone(), transition.consumer);
        let current = items.get_mut(&key).ok_or_else(|| {
            format!(
                "{}:{} references unknown event {}",
                path.display(),
                index + 1,
                transition.event_id
            )
        })?;
        if transition.idempotency_key != current.idempotency_key
            || transition.attempt < current.attempt
        {
            return Err(format!(
                "{}:{} violates outbox ordering",
                path.display(),
                index + 1
            ));
        }
        current.attempt = transition.attempt;
        current.lease_until = transition.lease_until;
        current.state = transition.state;
    }
    Ok(items)
}

#[allow(dead_code)]
fn append_transition(dir: &Path, transition: &OutboxTransition) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(OUTBOX_STATE_FILE))
        .map_err(|e| e.to_string())?;
    let mut encoded = serde_json::to_vec(transition).map_err(|e| e.to_string())?;
    encoded.push(b'\n');
    file.write_all(&encoded).map_err(|e| e.to_string())?;
    file.sync_data().map_err(|e| e.to_string())
}

#[allow(dead_code)]
fn bound_error(error: &str) -> String {
    error.chars().take(512).collect()
}
