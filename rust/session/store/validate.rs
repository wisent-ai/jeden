//! The check every ledger line has to pass before it is believed.
//!
//! Split out of `session/store.rs`, which had grown past the module line cap.

use super::super::event::{SessionEventV2, SessionPayloadV2};
use super::super::outbox::{OutboxConsumer, OutboxItem};
use super::active_lineage;
use std::path::Path;

pub(super) fn validate_next(
    path: &Path,
    line: usize,
    prior: &[SessionEventV2],
    event: &SessionEventV2,
    session_id: &str,
) -> Result<(), String> {
    let expected_sequence = prior.len() as u64 + 1;
    let active_leaf = prior.last().map(|entry| entry.event_id.as_str());
    if event.sequence != expected_sequence {
        return Err(format!(
            "{}:{} breaks ledger sequence: {}, expected {}",
            path.display(),
            line,
            event.sequence,
            expected_sequence
        ));
    }
    if prior.iter().any(|entry| entry.event_id == event.event_id) {
        return Err(format!(
            "{}:{} duplicates event id {}",
            path.display(),
            line,
            event.event_id
        ));
    }
    if matches!(event.payload, SessionPayloadV2::Rewind(_)) {
        let rewind = event.payload.rewind_data().map_err(|error| {
            format!(
                "{}:{} has invalid rewind payload: {}",
                path.display(),
                line,
                error
            )
        })?;
        if Some(rewind.from_leaf_id.as_str()) != active_leaf {
            return Err(format!(
                "{}:{} rewind source {} is not active leaf {:?}",
                path.display(),
                line,
                rewind.from_leaf_id,
                active_leaf
            ));
        }
        if event.parent_id.as_deref() != Some(rewind.checkpoint_id.as_str()) {
            return Err(format!(
                "{}:{} rewind parent {:?} does not match checkpoint {}",
                path.display(),
                line,
                event.parent_id,
                rewind.checkpoint_id
            ));
        }
        let target = prior
            .iter()
            .find(|entry| entry.event_id == rewind.checkpoint_id)
            .ok_or_else(|| {
                format!(
                    "{}:{} checkpoint not found: {}",
                    path.display(),
                    line,
                    rewind.checkpoint_id
                )
            })?;
        if !matches!(target.payload, SessionPayloadV2::Checkpoint(_)) {
            return Err(format!(
                "{}:{} event {} is not a checkpoint",
                path.display(),
                line,
                rewind.checkpoint_id
            ));
        }
        let ancestry = active_lineage(prior, active_leaf)?;
        if !ancestry
            .iter()
            .any(|entry| entry.event_id == rewind.checkpoint_id)
        {
            return Err(format!(
                "{}:{} checkpoint {} is not an ancestor of active leaf {:?}",
                path.display(),
                line,
                rewind.checkpoint_id,
                active_leaf
            ));
        }
    } else if event.parent_id.as_deref() != active_leaf {
        return Err(format!(
            "{}:{} breaks ledger lineage: parent {:?}, active leaf {:?}",
            path.display(),
            line,
            event.parent_id,
            active_leaf
        ));
    }
    if event.session_id != session_id {
        return Err(format!(
            "{}:{} belongs to session {}, expected {}",
            path.display(),
            line,
            event.session_id,
            session_id
        ));
    }
    if event.causation_id != event.parent_id {
        return Err(format!("{}:{} has invalid causation", path.display(), line));
    }
    if event.correlation_id.is_empty() {
        return Err(format!(
            "{}:{} has empty correlation id",
            path.display(),
            line
        ));
    }
    let valid_outbox = event.outbox.len() == OutboxConsumer::ALL.len()
        && OutboxConsumer::ALL.iter().all(|consumer| {
            let expected = OutboxItem::pending(*consumer, &event.event_id);
            event.outbox.iter().any(|item| item == &expected)
        });
    if !valid_outbox {
        return Err(format!(
            "{}:{} has invalid transactional outbox seeds",
            path.display(),
            line
        ));
    }
    Ok(())
}
