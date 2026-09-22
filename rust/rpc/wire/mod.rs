//! What one remote connection is before any session exists: who proved they
//! may speak, whose data they may touch, and how a request is carried.
//!
//! Grouped here so the `rpc` folder keeps to five entries; the module above
//! re-exports these under the names callers already use.

pub mod tenant;
pub mod tls;
pub mod transport;
