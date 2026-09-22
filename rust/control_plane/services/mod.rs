//! The services this harness talks to, and what is built on top of them:
//! Brama for model routing and its catalogue, Weles for accounts, providers,
//! billing and long operations, the quota figure an operator reads, and the
//! staging check that drives both before a release is trusted.
//!
//! Grouped here so the `control_plane` folder keeps to five entries; the
//! module above re-exports these under the names callers already use.

pub(crate) mod brama;
pub(crate) mod quota;
pub(crate) mod staging;
pub(crate) mod weles;

// The shared vocabulary each service is written against, under the names they
// have always used: a request's shape, how it is carried, what billing calls
// things, and the health a service reports.
use super::{billing, contract, now_ms, transport, ServiceHealth};
