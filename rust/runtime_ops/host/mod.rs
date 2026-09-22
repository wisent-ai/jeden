//! What a running operation can reach on the machine underneath it, each
//! gated by the same execution grant: the filesystem, the network, a managed
//! child process, and a pseudo-terminal.
//!
//! Grouped here so the `runtime_ops` folder keeps to five entries; the module
//! above re-exports these under the names callers already use.
use crate::tool_runtime::runtime_ops::security::GrantError;
use crate::tool_runtime::runtime_ops::untrusted_child;
use crate::tool_runtime::runtime_ops::SecureRuntime;

pub mod fs;
pub mod network;
pub mod process;
pub mod pty;
