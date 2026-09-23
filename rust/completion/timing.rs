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

use super::model::{CompletionState, WorkRequest};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
/// How much of a request's prompt names it when one answer reports several.
const EXCERPT_CHARS: usize = 60;

/// Seconds since the Unix epoch, the unit of every `*At` stamp in the state.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn epoch(stamp: &str) -> Option<u64> {
    stamp.trim().parse().ok()
}

fn signed(seconds: u64) -> i64 {
    i64::try_from(seconds).unwrap_or(i64::MAX)
}

/// A span of time the way every surface prints it: the two largest units,
/// each counted down, so `3 h 20 min` never reads as `3.3 h`. Under an hour
/// the seconds stay, because the time taken and the difference are printed
/// side by side and a first run printed `done in 1 min, 3 min under` a
/// 5 min estimate: 1 min 45 s and 3 min 15 s, each cut to whole minutes.
pub(crate) fn duration(seconds: u64) -> String {
    if seconds < MINUTE {
        format!("{seconds} s")
    } else if seconds < HOUR {
        match seconds % MINUTE {
            0 => format!("{} min", seconds / MINUTE),
            rest => format!("{} min {rest} s", seconds / MINUTE),
        }
    } else if seconds < DAY {
        match (seconds % HOUR) / MINUTE {
            0 => format!("{} h", seconds / HOUR),
            minutes => format!("{} h {minutes} min", seconds / HOUR),
        }
    } else {
        match (seconds % DAY) / HOUR {
            0 => format!("{} d", seconds / DAY),
            hours => format!("{} d {hours} h", seconds / DAY),
        }
    }
}

/// What became of a request that carries an estimate.
#[derive(Clone, Copy)]
enum Outcome {
    Open,
    Done {
        completed_at: u64,
        elapsed: u64,
    },
    /// Closed without an accepted review: every task was cancelled.
    Cancelled,
}

struct Entry<'a> {
    request: &'a WorkRequest,
    minutes: u64,
    estimated_at: u64,
    outcome: Outcome,
}

impl<'a> Entry<'a> {
    fn of(request: &'a WorkRequest) -> Option<Self> {
        let estimate = request.estimate.as_ref()?;
        let estimated_at = epoch(&estimate.recorded_at)?;
        let outcome = match request.completed_at.as_deref().and_then(epoch) {
            Some(completed_at) => Outcome::Done {
                completed_at,
                elapsed: completed_at.saturating_sub(estimated_at),
            },
            None if request.coverage_verified => Outcome::Cancelled,
            None => Outcome::Open,
        };
        Some(Self {
            request,
            minutes: estimate.minutes,
            estimated_at,
            outcome,
        })
    }

    fn estimate_seconds(&self) -> u64 {
        self.minutes.saturating_mul(MINUTE)
    }

    fn due_at(&self) -> u64 {
        self.estimated_at.saturating_add(self.estimate_seconds())
    }

    /// Actual minus estimated seconds: positive when the work took longer.
    fn difference(&self, elapsed: u64) -> i64 {
        signed(elapsed).saturating_sub(signed(self.estimate_seconds()))
    }

    fn ratio(&self, elapsed: u64) -> f64 {
        let ratio = elapsed as f64 / self.estimate_seconds() as f64;
        (ratio * 100.0).round() / 100.0
    }

    fn state(&self) -> &'static str {
        match self.outcome {
            Outcome::Open => "open",
            Outcome::Cancelled => "cancelled",
            Outcome::Done { elapsed, .. } if elapsed <= self.estimate_seconds() => "on_time",
            Outcome::Done { .. } => "late",
        }
    }

    fn value(&self) -> Value {
        let done = match self.outcome {
            Outcome::Done { elapsed, .. } => Some(elapsed),
            Outcome::Open | Outcome::Cancelled => None,
        };
        json!({
            "requestId": self.request.id,
            "estimateMinutes": self.minutes,
            "estimatedAt": self.estimated_at.to_string(),
            "dueAt": self.due_at().to_string(),
            "completedAt": done.and(self.request.completed_at.clone()),
            "elapsedSeconds": done,
            "differenceSeconds": done.map(|elapsed| self.difference(elapsed)),
            "ratio": done.map(|elapsed| self.ratio(elapsed)),
            "state": self.state(),
        })
    }

    fn line(&self, now: u64) -> String {
        let estimate = format!(
            "Time to completion: estimated {}",
            duration(self.estimate_seconds())
        );
        match self.outcome {
            Outcome::Open => {
                let spent = duration(now.saturating_sub(self.estimated_at));
                let due = self.due_at();
                if now <= due {
                    format!("{estimate} · {spent} so far, {} left", duration(due - now))
                } else {
                    format!(
                        "{estimate} · {spent} so far, {} past the estimate",
                        duration(now - due)
                    )
                }
            }
            Outcome::Cancelled => format!("{estimate} · cancelled before completion"),
            Outcome::Done { elapsed, .. } => format!(
                "{estimate} · done in {}, {}",
                duration(elapsed),
                self.verdict(elapsed, false)
            ),
        }
    }

    /// How the measured time compares with the estimate. Exact to the
    /// second, so the words never contradict the `on_time`/`late` state.
    fn verdict(&self, elapsed: u64, polish: bool) -> String {
        let difference = self.difference(elapsed);
        let spread = duration(difference.unsigned_abs());
        let ratio = self.ratio(elapsed);
        match (difference.cmp(&0), polish) {
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

    fn sentence(&self, elapsed: u64, several: bool, polish: bool) -> String {
        let estimate = duration(self.estimate_seconds());
        let took = duration(elapsed);
        let verdict = self.verdict(elapsed, polish);
        let subject = several.then(|| excerpt(&self.request.prompt));
        if polish {
            let head = match subject {
                Some(subject) => format!("Czas ukończenia „{subject}”"),
                None => "Czas ukończenia".to_string(),
            };
            format!("{head}: estymacja {estimate}, ukończono w {took}, {verdict}.")
        } else {
            let head = match subject {
                Some(subject) => format!("Time to completion of “{subject}”"),
                None => "Time to completion".to_string(),
            };
            format!("{head}: estimated {estimate}, done in {took}, {verdict}.")
        }
    }
}

fn excerpt(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or(prompt).trim();
    match line.char_indices().nth(EXCERPT_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
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
            .filter_map(Entry::of)
            .map(|entry| entry.value())
            .collect(),
    )
}

/// The time-to-completion line under a request in `jeden todo list`.
pub(crate) fn line(request: &WorkRequest, now: u64) -> String {
    match Entry::of(request) {
        Some(entry) => entry.line(now),
        None if !request.planned && !request.coverage_verified => {
            "Time to completion: not estimated yet; Jeden records the estimate before execution"
                .into()
        }
        None => "Time to completion: no estimate was recorded".into(),
    }
}

/// What the execution model is told about a retained request's time.
pub(crate) fn context(request: &WorkRequest, now: u64) -> Option<Value> {
    let entry = Entry::of(request)?;
    Some(json!({
        "estimateMinutes": entry.minutes,
        "minutesSinceEstimate": now.saturating_sub(entry.estimated_at) / MINUTE,
    }))
}

/// The sentences a final answer ends with: every request an independent
/// review accepted at or after `since`, measured against its first estimate.
pub(crate) fn report(state: &CompletionState, since: u64, polish: bool) -> Option<String> {
    let done: Vec<_> = state
        .requests
        .iter()
        .filter_map(Entry::of)
        .filter_map(|entry| match entry.outcome {
            Outcome::Done {
                completed_at,
                elapsed,
            } if completed_at >= since => Some((entry, elapsed)),
            _ => None,
        })
        .collect();
    let several = done.len() > 1;
    let sentences: Vec<_> = done
        .iter()
        .map(|(entry, elapsed)| entry.sentence(*elapsed, several, polish))
        .collect();
    (!sentences.is_empty()).then(|| sentences.join("\n"))
}
