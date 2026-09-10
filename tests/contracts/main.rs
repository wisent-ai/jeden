//! Product contract journeys through the real CLI and RPC binary.
//!
//! Model-dependent journeys use the configured Brama route and fail on its
//! actual refusal. Evidence and isolated state remain under
//! `target/contract-runs`; `home.rs` owns that isolation and `cases/` holds
//! the journeys themselves.
//!
//! Run: `npm run test:contracts`, which signs the binaries Cargo built before
//! executing the suite.

mod cases;
mod home;
