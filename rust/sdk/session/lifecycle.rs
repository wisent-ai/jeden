use super::*;

impl AgentSession {
    /// Seal admission only when no request is executing or queued.
    pub fn retire(&self) -> Result<(), String> {
        {
            let active = self.inner.active.lock().map_err(|error| {
                format!("cannot inspect active requests for retirement: {error}")
            })?;
            if !active.is_empty() {
                let mut requests: Vec<&str> = active.keys().map(String::as_str).collect();
                requests.sort_unstable();
                return Err(format!(
                    "cannot retire session with active requests: {}",
                    requests.join(", ")
                ));
            }
            self.inner.disposed.store(true, Ordering::Release);
        }
        self.dispose()
    }

    pub fn dispose(&self) -> Result<(), String> {
        // Close admission immediately. A previous cleanup failure is not success:
        // each caller must complete these idempotent cleanup operations itself.
        self.inner.disposed.store(true, Ordering::Release);
        // Disposal clears, cancels and releases whole values, so a holder that
        // panicked earlier does not leave a session that can never be
        // disposed: each lock is taken back, the conversation included, since
        // releasing it does not read what a panicking turn left in it.
        for cancel in crate::panicked::held(&self.inner.active).values() {
            cancel.store(true, Ordering::Release);
        }
        crate::tool_runtime::runtime_ops::announce_cancellation();
        crate::panicked::held(&self.inner.subscribers).clear();
        *crate::panicked::written(&self.inner.interactions) = None;
        *crate::panicked::held(&self.inner.conversation) = None;
        Ok(())
    }
}
