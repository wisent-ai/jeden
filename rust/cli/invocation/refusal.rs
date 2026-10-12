//! A command refuses its own invocation (an unexpected argument, a missing
//! confirmation) through `usage`, so `main` exits 2 for it and 1 for a failure
//! of a well-formed command (cli.md rule 10). Commands answer
//! `Result<String, String>`, so the kind travels beside the text: `usage`
//! keeps the exact refusal it produced and `is_usage` asks whether the error
//! `main` received is that refusal, never what its words say.

use std::sync::{Mutex, MutexGuard};

static REFUSAL: Mutex<Option<String>> = Mutex::new(None);

/// The slot is only ever replaced whole, so a poisoned lock is recovered: a
/// refusal it dropped would make `main` exit as if a well-formed command
/// had failed.
fn recorded() -> MutexGuard<'static, Option<String>> {
    match REFUSAL.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The refusal text, recorded as this process's usage refusal.
pub(crate) fn usage(text: impl Into<String>) -> String {
    let text = text.into();
    *recorded() = Some(text.clone());
    text
}

/// Whether `error` is the usage refusal a command produced through `usage`.
pub(crate) fn is_usage(error: &str) -> bool {
    recorded().as_deref() == Some(error)
}
