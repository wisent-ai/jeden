//! Both versions of the billing contract this client speaks, and the checks
//! they share.
pub(crate) use crate::control_plane::contract::negotiate;
pub(crate) use crate::control_plane::contract::negotiate_response;

mod billing;
pub(crate) mod guards;
mod legacy;
