//! A synchronous source carries its caller cancellation through nested memory
//! operations without changing unrelated users of the shared fleet client.
use std::cell::RefCell;
use std::sync::Arc;

use crate::tool_runtime::runtime_ops::context::StadoCancellation;
use crate::tool_runtime::runtime_ops::{wake_on_cancellation, CancellationToken};

thread_local! {
    static CURRENT: RefCell<Option<Arc<StadoCancellation>>> = const { RefCell::new(None) };
}

struct Scope(Option<Arc<StadoCancellation>>);

impl Drop for Scope {
    fn drop(&mut self) {
        drop(CURRENT.with(|current| current.replace(self.0.take())));
    }
}

pub(crate) fn with_cancellation<T>(token: &CancellationToken, work: impl FnOnce() -> T) -> T {
    let context = token.for_stado();
    let _scope = Scope(CURRENT.with(|current| current.replace(Some(context))));
    work()
}

pub(super) fn check() -> Result<(), String> {
    if CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .is_some_and(|context| context.caller.is_cancelled())
    }) {
        Err(super::failed(
            "running an operation",
            "cancelled by the caller",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn database() -> Option<stado_wait::Cancellation> {
    CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .map(|context| context.event.clone())
    })
}

pub(super) fn wake_when_cancelled(wake: impl Fn() + Send + 'static) -> Option<Arc<()>> {
    let token = CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .map(|context| context.caller.clone())
    })?;
    let alive = Arc::new(());
    let weak = Arc::downgrade(&alive);
    wake_on_cancellation(move || {
        let Some(_alive) = weak.upgrade() else {
            return false;
        };
        if token.is_cancelled() {
            wake();
        }
        true
    });
    Some(alive)
}
