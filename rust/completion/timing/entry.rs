//! One request's time to completion, read from its stamps: what the snapshot,
//! the todo list line and the final answer print about it.

use super::{duration, estimate_seconds, seconds};
use crate::completion::model::{CompletionEstimate, WorkRequest};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::num::NonZeroU64;

/// The time a verified request took, from its estimate.
pub(super) struct Measured {
    completed_at: u64,
    elapsed: u64,
    /// Actual minus estimated seconds: positive when the work took longer.
    difference: i64,
}

/// What became of a request that carries an estimate.
pub(super) enum Outcome {
    Open,
    Done(Measured),
    /// Verified complete, but its time cannot be measured; says why.
    Unmeasured {
        completed_at: u64,
        reason: String,
    },
    /// Closed without an accepted review: every task was cancelled.
    Cancelled,
}

pub(super) struct Entry<'a> {
    request: &'a WorkRequest,
    pub(super) minutes: NonZeroU64,
    estimate: u64,
    pub(super) estimated_at: u64,
    due_at: u64,
    outcome: Outcome,
}

impl<'a> Entry<'a> {
    pub(super) fn of(request: &'a WorkRequest) -> Option<Result<Self, String>> {
        let estimate = request.estimate.as_ref()?;
        Some(Self::read(request, estimate))
    }

    fn read(request: &'a WorkRequest, estimate: &CompletionEstimate) -> Result<Self, String> {
        let estimated_at = seconds(request, "estimate.recordedAt", &estimate.recorded_at)?;
        let seconds_estimated = estimate_seconds(estimate)?;
        let due_at = estimated_at.checked_add(seconds_estimated).ok_or_else(|| {
            format!(
                "request {} is due {} after {estimated_at}, later than Jeden can count",
                request.id,
                duration(seconds_estimated)
            )
        })?;
        let outcome = match &request.completed_at {
            Some(stamp) => {
                let completed_at = seconds(request, "completedAt", stamp)?;
                match measure(request, completed_at, estimated_at, seconds_estimated) {
                    Ok(measured) => Outcome::Done(measured),
                    Err(reason) => Outcome::Unmeasured {
                        completed_at,
                        reason,
                    },
                }
            }
            None if request.coverage_verified => Outcome::Cancelled,
            None => Outcome::Open,
        };
        Ok(Self {
            request,
            minutes: estimate.minutes,
            estimate: seconds_estimated,
            estimated_at,
            due_at,
            outcome,
        })
    }

    pub(super) fn completed_at(&self) -> Option<u64> {
        match &self.outcome {
            Outcome::Done(measured) => Some(measured.completed_at),
            Outcome::Unmeasured { completed_at, .. } => Some(*completed_at),
            Outcome::Open | Outcome::Cancelled => None,
        }
    }

    fn ratio(&self, elapsed: u64) -> f64 {
        elapsed as f64 / self.estimate as f64
    }

    pub(super) fn value(&self) -> Value {
        let (measured, state, reason) = match &self.outcome {
            Outcome::Open => (None, "open", None),
            Outcome::Cancelled => (None, "cancelled", None),
            Outcome::Done(measured) if measured.elapsed <= self.estimate => {
                (Some(measured), "on_time", None)
            }
            Outcome::Done(measured) => (Some(measured), "late", None),
            Outcome::Unmeasured { reason, .. } => (None, "unmeasurable", Some(reason)),
        };
        json!({
            "requestId": self.request.id,
            "estimateMinutes": self.minutes,
            "estimatedAt": self.estimated_at.to_string(),
            "dueAt": self.due_at.to_string(),
            "completedAt": self.request.completed_at,
            "elapsedSeconds": measured.map(|measured| measured.elapsed),
            "differenceSeconds": measured.map(|measured| measured.difference),
            "ratio": measured.map(|measured| self.ratio(measured.elapsed)),
            "state": state,
            "reason": reason,
        })
    }

    pub(super) fn line(&self, now: u64) -> String {
        let estimate = format!("Time to completion: estimated {}", duration(self.estimate));
        match &self.outcome {
            Outcome::Open => match now.checked_sub(self.estimated_at) {
                None => format!(
                    "{estimate} · the clock reads {now}, before the estimate was recorded at {}",
                    self.estimated_at
                ),
                Some(spent) => match self.due_at.checked_sub(now) {
                    Some(left) => format!(
                        "{estimate} · {} so far, {} left",
                        duration(spent),
                        duration(left)
                    ),
                    None => format!(
                        "{estimate} · {} so far, {} past the estimate",
                        duration(spent),
                        duration(now - self.due_at)
                    ),
                },
            },
            Outcome::Cancelled => format!("{estimate} · cancelled before completion"),
            Outcome::Done(measured) => format!(
                "{estimate} · done in {}, {}",
                duration(measured.elapsed),
                self.verdict(measured, false)
            ),
            Outcome::Unmeasured { reason, .. } => {
                format!("{estimate} · verified complete, not measurable: {reason}")
            }
        }
    }

    /// How the measured time compares with the estimate. Exact to the
    /// second, so the words never contradict the `on_time`/`late` state.
    fn verdict(&self, measured: &Measured, polish: bool) -> String {
        let spread = duration(measured.elapsed.abs_diff(self.estimate));
        let ratio = self.ratio(measured.elapsed);
        match (measured.elapsed.cmp(&self.estimate), polish) {
            (Ordering::Equal, false) => "as estimated".into(),
            (Ordering::Less, false) => format!("{spread} under the estimate"),
            (Ordering::Greater, false) => {
                format!("{spread} over the estimate ({ratio:.2}× the estimate)")
            }
            (Ordering::Equal, true) => "zgodnie z estymacją".into(),
            (Ordering::Less, true) => format!("{spread} poniżej estymacji"),
            (Ordering::Greater, true) => {
                format!("{spread} ponad estymację ({ratio:.2}× estymacji)")
            }
        }
    }

    pub(super) fn sentence(&self, several: bool, polish: bool) -> String {
        let estimate = duration(self.estimate);
        let subject = several.then(|| first_line(&self.request.prompt));
        let tail = match (&self.outcome, polish) {
            (Outcome::Done(measured), false) => {
                format!(
                    "done in {}, {}",
                    duration(measured.elapsed),
                    self.verdict(measured, false)
                )
            }
            (Outcome::Done(measured), true) => {
                format!(
                    "ukończono w {}, {}",
                    duration(measured.elapsed),
                    self.verdict(measured, true)
                )
            }
            (Outcome::Unmeasured { reason, .. }, false) => {
                format!("verified complete, not measurable: {reason}")
            }
            (Outcome::Unmeasured { reason, .. }, true) => {
                format!("ukończono, czasu nie można zmierzyć: {reason}")
            }
            (Outcome::Open, false) => "not completed yet".into(),
            (Outcome::Open, true) => "jeszcze nieukończone".into(),
            (Outcome::Cancelled, false) => "cancelled before completion".into(),
            (Outcome::Cancelled, true) => "anulowano przed ukończeniem".into(),
        };
        match (subject, polish) {
            (Some(subject), true) => {
                format!("Czas ukończenia „{subject}”: estymacja {estimate}, {tail}.")
            }
            (None, true) => format!("Czas ukończenia: estymacja {estimate}, {tail}."),
            (Some(subject), false) => {
                format!("Time to completion of “{subject}”: estimated {estimate}, {tail}.")
            }
            (None, false) => format!("Time to completion: estimated {estimate}, {tail}."),
        }
    }
}

/// The time a request took from its estimate to its verified completion; a
/// completion stamped before the estimate, which only a clock set back
/// between the two can produce, is refused with both stamps.
fn measure(
    request: &WorkRequest,
    completed_at: u64,
    estimated_at: u64,
    estimate: u64,
) -> Result<Measured, String> {
    let elapsed = completed_at.checked_sub(estimated_at).ok_or_else(|| {
        format!(
            "request {} was verified complete at {completed_at}, before its estimate was recorded at {estimated_at}: the clock was set back between them",
            request.id
        )
    })?;
    let difference = i64::try_from(i128::from(elapsed) - i128::from(estimate)).map_err(|error| {
        format!(
            "request {} took {elapsed} s against an estimate of {estimate} s, a difference Jeden cannot count: {error}",
            request.id
        )
    })?;
    Ok(Measured {
        completed_at,
        elapsed,
        difference,
    })
}

/// A request is named, when one answer reports several, by the first line of
/// its prompt, whole.
fn first_line(prompt: &str) -> &str {
    match prompt.split_once('\n') {
        Some((first, _rest)) => first.trim(),
        None => prompt.trim(),
    }
}
