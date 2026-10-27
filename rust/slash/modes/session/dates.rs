//! Reading the moment a session started, and saying how long ago that was.
//!
//! Split out of `slash/modes/session.rs`, which had grown past the module line
//! cap.

use jiff::{SpanRound, Timestamp, Unit};

/// The moment `started` names: an RFC 3339 instant, or whole epoch seconds as
/// earlier sessions wrote it; anything else is refused naming the value.
fn started_at(started: &str) -> Result<Timestamp, String> {
    let trimmed = started.trim();
    match trimmed.parse::<i64>() {
        Ok(seconds) => Timestamp::from_second(seconds)
            .map_err(|error| format!("{trimmed} is not an instant: {error}")),
        Err(_not_epoch_seconds) => trimmed.parse::<Timestamp>().map_err(|error| {
            format!("{trimmed:?} is neither epoch seconds nor an RFC 3339 instant: {error}")
        }),
    }
}

/// How long ago `started` was, in days, hours, minutes and seconds; a start
/// that cannot be read says why instead of showing an age.
pub(super) fn relative_age(started: &str) -> String {
    let at = match started_at(started) {
        Ok(at) => at,
        Err(cause) => return format!("age unreadable: {cause}"),
    };
    let rounding = SpanRound::new()
        .largest(Unit::Day)
        .smallest(Unit::Second)
        .days_are_24_hours();
    match Timestamp::now().since(at).and_then(|span| span.round(rounding)) {
        Ok(span) => format!("{span:#}"),
        Err(error) => format!("age unreadable: {error}"),
    }
}
