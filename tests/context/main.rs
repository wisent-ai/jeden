//! The context advisor, driven through the real `jeden` binary.
//!
//! Three stories, one per surface the operator asked for: the command a
//! person runs, the tool an Omp session calls, and the block a turn receives
//! without asking. Every case seeds its own documentation corpus and its own
//! isolated home, so nothing here reads or writes the operator's memory
//! store, sessions, or Omp tool directory.
//!
//! Run: `npm run test:context`, which builds and signs the binaries before
//! executing them. Runs keep their state under `target/context-runs`.

mod cli;
mod fixture;
mod omp;
mod prologue;
