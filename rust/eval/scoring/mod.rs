//! How an evaluation run is judged and written down: the graders, the
//! metrics they produce, and the canonical report that quotes both.
//!
//! Grouped here so the `eval` folder keeps to five entries; the module above
//! re-exports these under the names callers already use.

pub mod graders;
pub mod metrics;
pub mod report;
