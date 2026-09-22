//! What this workspace knows about its external tool servers before any of
//! them is contacted.

mod capabilities;
mod config;

pub(crate) use capabilities::capability_descriptors;
pub use config::load_config;
pub(crate) use config::{configured_server, configured_servers, resolve_server_cwd, string_field};
