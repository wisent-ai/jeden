//! Time to completion: the estimate a request received before execution,
//! beside how long it took until it was independently verified complete.
//!
//! The estimate is recorded once, when the intake plans the request, and
//! nothing revises it. The completion time is written only by an accepted
//! independent review and removed again when a defect reopens the request.
//! What is compared is therefore the first promise against the moment the
//! work was really done, never a later guess against an earlier claim. The
//! snapshot, `jeden todo list`, the model's context and the final answer all
//! read it from here, and the graphical Tasks panels print the same words.
//!
//! A stamp that is not Unix seconds is refused whenever the completion state
//! is read or written ([`check`]). A completion stamped before its estimate
//! is reported as not measurable with both stamps, never measured as zero.

mod entry;

use super::model::{CompletionEstimate, CompletionState, WorkRequest};
use entry::Entry;
use serde_json::{json, Value};
use std::num::NonZeroU64;

/// Seconds in a minute, a unit accepted for use with the SI (SI Brochure,
/// Table 8): https://www.bipm.org/en/publications/si-brochure
const MINUTE: u64 = 60;
/// Seconds in an hour (SI Brochure, Table 8):
/// https://www.bipm.org/en/publications/si-brochure
const HOUR: u64 = 60 * MINUTE;
/// Seconds in a day (SI Brochure, Table 8):
/// https://www.bipm.org/en/publications/si-brochure
const DAY: u64 = 24 * HOUR;

/// Seconds since the Unix epoch, the unit of every `*At` stamp in the state;
/// a clock before the epoch is refused, never read as time zero.
pub(crate) fn now() -> Result<u64, String> {
    crate::clock::unix_seconds().map_err(|error| error.to_string())
}

/// The Unix seconds written in `stamp`; anything else is refused naming the
/// request and the field that hold it.
fn seconds(request: &WorkRequest, field: &str, stamp: &str) -> Result<u64, String> {
    stamp.trim().parse().map_err(|error| {
        format!(
            "request {} records {field} {stamp:?}, which is not whole seconds since the Unix epoch: {error}",
            request.id
        )
    })
}

/// The span an estimate of whole minutes stands for, refused when it is more
/// seconds than can be counted.
pub(crate) fn estimate_seconds(estimate: &CompletionEstimate) -> Result<u64, String> {
    estimate.minutes.get().checked_mul(MINUTE).ok_or_else(|| {
        format!(
            "an estimate of {} minutes is more seconds than Jeden can count",
            estimate.minutes
        )
    })
}

/// Refuses a request whose recorded time cannot be read: a stamp that is not
/// Unix seconds, or an estimate whose due time cannot be counted. The
/// completion state runs this whenever it is read or written.
pub(crate) fn check(request: &WorkRequest) -> Result<(), String> {
    if let Some(stamp) = &request.completed_at {
        seconds(request, "completedAt", stamp)?;
    }
    Entry::of(request).transpose().map(drop)
}

/// A span of time the way every surface prints it: the two largest units,
/// each counted down, so hours and minutes never read as a decimal of hours.
/// Under an hour the seconds stay, because the time taken and the difference
/// are printed side by side, and cutting both to whole minutes made their sum
/// disagree with the estimate printed beside them.
pub(crate) fn duration(seconds: u64) -> String {
    if seconds < MINUTE {
        format!("{seconds} s")
    } else if seconds < HOUR {
        span(seconds / MINUTE, "min", seconds % MINUTE, "s")
    } else if seconds < DAY {
        span(seconds / HOUR, "h", (seconds % HOUR) / MINUTE, "min")
    } else {
        span(seconds / DAY, "d", (seconds % DAY) / HOUR, "h")
    }
}

/// `whole` of the larger unit, then `rest` of the smaller one when any is left.
fn span(whole: u64, unit: &str, rest: u64, smaller: &str) -> String {
    match NonZeroU64::new(rest) {
        Some(rest) => format!("{whole} {unit} {rest} {smaller}"),
        None => format!("{whole} {unit}"),
    }
}

/// The snapshot's `timing` array: one entry per request that carries an
/// estimate, in request order. Everything in it follows from the persisted
/// state alone; a client works out time still running from `estimatedAt`.
pub(crate) fn values(state: &CompletionState) -> Value {
    Value::Array(
        state
            .requests
            .iter()
            .filter_map(|request| {
                Entry::of(request).map(|entry| match entry {
                    Ok(entry) => entry.value(),
                    Err(reason) => {
                        json!({"requestId": request.id, "state": "unreadable", "reason": reason})
                    }
                })
            })
            .collect(),
    )
}

/// The time-to-completion line under a request in `jeden todo list`.
pub(crate) fn line(request: &WorkRequest, now: u64) -> String {
    match Entry::of(request) {
        Some(Ok(entry)) => entry.line(now),
        Some(Err(reason)) => format!("Time to completion: cannot be read: {reason}"),
        None if !request.planned && !request.coverage_verified => {
            "Time to completion: not estimated yet; Jeden records the estimate before execution"
                .into()
        }
        None => "Time to completion: no estimate was recorded".into(),
    }
}

/// What the execution model is told about a retained request's time.
pub(crate) fn context(request: &WorkRequest, now: u64) -> Option<Value> {
    Some(match Entry::of(request)? {
        Ok(entry) => match now.checked_sub(entry.estimated_at) {
            Some(spent) => {
                json!({"estimateMinutes": entry.minutes, "minutesSinceEstimate": spent / MINUTE})
            }
            None => json!({
                "estimateMinutes": entry.minutes,
                "unmeasurable": format!(
                    "the clock reads {now}, before the estimate was recorded at {}",
                    entry.estimated_at
                ),
            }),
        },
        Err(reason) => json!({"unreadable": reason}),
    })
}

/// The sentences a final answer ends with: every request verified complete
/// at or after `since`, measured against its first estimate, and every
/// request whose recorded time cannot be read.
pub(crate) fn report(state: &CompletionState, since: u64, polish: bool) -> Option<String> {
    let entries: Vec<_> = state.requests.iter().filter_map(Entry::of).collect();
    let reported: Vec<_> = entries
        .iter()
        .filter(|entry| match entry {
            Ok(entry) => entry.completed_at().is_some_and(|at| at >= since),
            Err(_reason) => true,
        })
        .collect();
    let several = matches!(reported.as_slice(), [_, _, ..]);
    let sentences: Vec<_> = reported
        .iter()
        .map(|entry| match entry {
            Ok(entry) => entry.sentence(several, polish),
            Err(reason) if polish => format!("Czasu ukończenia nie można odczytać: {reason}."),
            Err(reason) => format!("Time to completion cannot be read: {reason}."),
        })
        .collect();
    (!sentences.is_empty()).then(|| sentences.join("\n"))
}
