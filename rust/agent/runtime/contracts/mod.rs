//! What the agent is told it owes, in whichever language the operator set:
//! the default communication contract, the translated prose that carries it,
//! and the task contract used for prompts, settings, reports and diagnostics.
//!
//! Grouped here so the `runtime` folder keeps to five entries; the module
//! above re-exports these under the names callers already use.

pub(crate) mod communication_contract;
pub(crate) mod language_prose;
pub(crate) mod task_contract;
