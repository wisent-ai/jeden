//! The commands that serve the developer rather than the session: the shell
//! completions this binary emits for itself, the component gallery, and the
//! token counter.
//!
//! Grouped here so the `cli` folder keeps to five entries; the module above
//! re-exports them under the names callers already use.

pub(crate) mod completions;
pub(crate) mod gallery;
pub(crate) mod token;
