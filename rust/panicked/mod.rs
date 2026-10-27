//! What a thread or call that panicked hands back, said as text.
//! `JoinHandle::join` and `catch_unwind` answer the panic's payload: a
//! `String` when the panic formatted its message, a `&'static str` when it
//! was a literal, and any other type when it was raised with
//! `std::panic::panic_any` (https://doc.rust-lang.org/std/thread/type.Result.html).
//! Every refusal that reports a panic reads the payload here, so none drops
//! the message and none decodes it a second way; and a lock a panic poisoned
//! is entered here, so none turns one panic into a refusal of every later
//! caller.

use std::any::Any;
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// The guard of `lock`, taken back after a holder panicked, its poison
/// cleared. Only for a value every holder changes by whole steps (one insert,
/// remove, take or replace, or no value at all), so what a panicking holder
/// left is still a state; refused instead, one panic refused every later
/// caller until the process ended.
pub(crate) fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            poisoned.into_inner()
        }
    }
}

/// The read guard of `lock`, taken back the way [`held`] takes a mutex.
pub(crate) fn read_held<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            poisoned.into_inner()
        }
    }
}

/// The write guard of `lock`, taken back the way [`held`] takes a mutex.
pub(crate) fn written<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            poisoned.into_inner()
        }
    }
}

/// The message `payload` carries, or the type of a payload that carries
/// none (dispatched through the payload's own vtable, so it is the type the
/// panic was raised with, not `dyn Any`).
pub(crate) fn message(payload: &(dyn Any + Send)) -> String {
    match (
        payload.downcast_ref::<String>(),
        payload.downcast_ref::<&'static str>(),
    ) {
        (Some(text), _) => text.clone(),
        (None, Some(text)) => (*text).to_string(),
        (None, None) => format!(
            "a panic payload of type {:?} that carries no text",
            (*payload).type_id()
        ),
    }
}

/// `what` panicked, with the message its payload carries.
pub(crate) fn refusal(what: &str, payload: &(dyn Any + Send)) -> String {
    format!("{what} panicked: {}", message(payload))
}
