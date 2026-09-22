//! What every slash command reads and writes underneath: the merged
//! configuration, the mode state file, and the name checks that refuse an
//! argument before it reaches a command line.
//!
//! Grouped here so the `slash` folder keeps to five entries; the module above
//! re-exports these under the names callers already use.

pub(crate) mod common;
pub(crate) mod state;
pub(crate) mod validate;
