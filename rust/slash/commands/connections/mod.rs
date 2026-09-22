//! The two slash commands that manage a connection to something outside this
//! session: the model context protocol servers it may talk to, and the secure
//! shell hosts it may reach.
//!
//! Grouped here so the `commands` folder keeps to five entries; the module
//! above re-exports both under the names callers already use.

pub(crate) mod mcp;
pub(crate) mod ssh;
