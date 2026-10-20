//! Counting: the one place Jeden states the step of a count, so a count that
//! adds or takes one item, and a numbering that starts at its first item,
//! name that step instead of spelling it at each site.

use std::sync::atomic::{AtomicU64, Ordering};

/// The step of a count: each item adds one, as `Iterator::count` counts
/// (<https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.count>).
pub(crate) const STEP: u8 = 1;

/// The number the first item of a numbering takes: one step from none.
pub(crate) const FIRST: u64 = STEP as u64;

/// The next number a shared counter hands out, counting this draw: the first
/// draw answers the counter's starting value.
pub(crate) fn draw(counter: &AtomicU64) -> u64 {
    counter.fetch_add(u64::from(STEP), Ordering::Relaxed)
}
