//! The one reader of wall-clock time since the Unix epoch. A host clock set
//! before 1970 has no time to give: it is refused with that sentence, never
//! read as the epoch itself, so no record is stamped zero and sorted first.

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Time since the Unix epoch, or the refusal of a clock set before it.
pub fn since_epoch() -> io::Result<Duration> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            io::Error::other(format!(
                "the system clock stands before the Unix epoch, so no time can be stamped: {error}"
            ))
        })
}

/// Whole seconds since the Unix epoch.
pub fn unix_seconds() -> io::Result<u64> {
    since_epoch().map(|elapsed| elapsed.as_secs())
}

/// Milliseconds since the Unix epoch; a count a u64 cannot hold is refused.
pub fn unix_millis() -> io::Result<u64> {
    let millis = since_epoch()?.as_millis();
    u64::try_from(millis).map_err(|error| {
        io::Error::other(format!(
            "the system clock reads {millis} ms since the Unix epoch, more than a u64 holds: {error}"
        ))
    })
}
