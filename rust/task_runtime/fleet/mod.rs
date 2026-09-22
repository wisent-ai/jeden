//! How work is handed to a worker and taken back: the negotiated protocol,
//! the placement that chooses a worker, the coordinator that leases an
//! attempt, the store that survives a restart, and the worker runtime.
//!
//! Grouped here so the `task_runtime` folder keeps to five entries; the
//! module above re-exports these under the names callers already use.

pub mod coordinator;
pub mod protocol;
pub mod worker;

pub use coordinator::{placement, store};
