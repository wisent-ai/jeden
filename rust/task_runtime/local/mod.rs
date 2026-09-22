//! What a task needs on this machine before it can run: the agent
//! definitions, the mailbox it speaks through, the isolated workspace it is
//! given, and the sandbox every spawned process goes through.
//!
//! Grouped here so the `task_runtime` folder keeps to five entries; the
//! module above re-exports these under the names callers already use.

pub mod discovery;
pub mod mailbox;
pub mod sandbox;
pub mod workspace;
