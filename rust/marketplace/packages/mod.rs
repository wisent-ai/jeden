//! What a marketplace package is: what a release declares, what a lock
//! records, and whose signature this installation accepts.
//!
//! Grouped here so the `marketplace` folder keeps to five entries; the module
//! above re-exports these under the names the resolver and the service
//! already use.

pub mod lock;
pub mod manifest;
pub mod trust;
