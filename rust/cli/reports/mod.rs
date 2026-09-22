//! The commands that only read and print: billing and quota, runtime
//! statistics, the sessions on disk, and the contract conformance report.
//!
//! None of the four writes anything. Grouped here so the `cli` folder keeps
//! to five entries; the module above re-exports them under the names callers
//! already use.

pub(crate) mod billing;
pub(crate) mod contracts;
pub(crate) mod sessions;
pub(crate) mod stats;
