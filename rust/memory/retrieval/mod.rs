//! How a memory is found again: the embeddings it is stored under, and the
//! ranking that puts a full-text match and a semantic one together.
//!
//! Grouped here so the `memory` folder keeps to five entries; the module
//! above re-exports both under the names callers already use.

pub mod embeddings;
pub mod ranking;
