//! The one place that hands back an HTTP client carrying no limit of its own.
//!
//! `reqwest`'s blocking client cuts every request off after thirty seconds
//! unless it is told not to. Telling it at each of the thirteen places Jeden
//! builds a client meant every call site named a number, and every new call
//! site had to remember to name one. This says it once, and every caller
//! starts from a builder that carries nothing.
//!
//! A request that needs to stop early stops on something that happened — the
//! operator cancelled, the stream ended, the process exited — never on a clock
//! that ran out while the other side was still answering.

/// A blocking client builder with `reqwest`'s own thirty seconds cleared.
pub fn blocking_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder().timeout(None)
}
