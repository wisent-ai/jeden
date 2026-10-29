//! Exclusive execution ownership without holding a registry across native I/O.
use super::OperationContext;
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

pub(crate) trait PersistentProcess: Send + 'static {
    fn retire(&mut self, cancelled: bool);
}

struct State<T> {
    busy: bool,
    retired: bool,
    process: Option<T>,
}

pub(crate) struct SessionSlot<K: Debug + Send + Sync + 'static, T: PersistentProcess> {
    pub(crate) key: K,
    label: &'static str,
    state: Mutex<State<T>>,
    ready: Condvar,
}

pub(crate) struct SessionLease<K: Debug + Send + Sync + 'static, T: PersistentProcess> {
    slot: Arc<SessionSlot<K, T>>,
    process: Option<T>,
}

impl<K: Debug + Send + Sync + 'static, T: PersistentProcess> SessionSlot<K, T> {
    pub(crate) fn new(key: K, label: &'static str) -> Arc<Self> {
        let slot = Arc::new(Self {
            key,
            label,
            state: Mutex::new(State {
                busy: false,
                retired: false,
                process: None,
            }),
            ready: Condvar::new(),
        });
        let weak = Arc::downgrade(&slot);
        super::wake_on_cancellation(move || {
            let Some(slot) = weak.upgrade() else {
                return false;
            };
            // Synchronize with the condition check before wait releases the lock.
            let state = slot.cleanup_state();
            slot.ready.notify_all();
            drop(state);
            true
        });
        slot
    }

    pub(crate) fn acquire(
        self: &Arc<Self>,
        context: &OperationContext<'_>,
    ) -> Result<SessionLease<K, T>, String> {
        let mut state = self.state.lock().map_err(|error| {
            format!(
                "{} {:?} command lock poisoned: {error}",
                self.label, self.key
            )
        })?;
        loop {
            if context.cancellation().is_cancelled() {
                return Err(format!(
                    "{} {:?} execution cancelled before command was written",
                    self.label, self.key
                ));
            }
            if state.retired {
                return Err(format!(
                    "{} {:?} was torn down before execution",
                    self.label, self.key
                ));
            }
            if !state.busy {
                state.busy = true;
                return Ok(SessionLease {
                    slot: Arc::clone(self),
                    process: state.process.take(),
                });
            }
            state = self.ready.wait(state).map_err(|error| {
                format!(
                    "{} {:?} command wait poisoned: {error}",
                    self.label, self.key
                )
            })?;
        }
    }

    fn cleanup_state(&self) -> MutexGuard<'_, State<T>> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                eprintln!("jeden: {} {:?} command lock poisoned during cleanup: {poisoned}; recovering its owned process", self.label, self.key);
                poisoned.into_inner()
            }
        }
    }

    pub(crate) fn close(&self) -> Option<T> {
        let mut state = self.cleanup_state();
        state.retired = true;
        self.ready.notify_all();
        state.process.take()
    }
}

impl<K: Debug + Send + Sync + 'static, T: PersistentProcess> Deref for SessionLease<K, T> {
    type Target = Option<T>;
    fn deref(&self) -> &Self::Target {
        &self.process
    }
}
impl<K: Debug + Send + Sync + 'static, T: PersistentProcess> DerefMut for SessionLease<K, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}
impl<K: Debug + Send + Sync + 'static, T: PersistentProcess> Drop for SessionLease<K, T> {
    fn drop(&mut self) {
        let mut state = self.slot.cleanup_state();
        let stop = state.retired || std::thread::panicking() || self.slot.state.is_poisoned();
        if stop {
            let cancelled = state.retired;
            // Keep the lease busy through cleanup, without holding this state lock.
            drop(state);
            if let Some(mut process) = self.process.take() {
                process.retire(cancelled);
            }
            state = self.slot.cleanup_state();
        } else {
            state.process = self.process.take();
        }
        state.busy = false;
        self.slot.ready.notify_all();
    }
}
