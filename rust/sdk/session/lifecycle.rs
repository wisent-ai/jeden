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
        for cancel in self
            .inner
            .active
            .lock()
            .map_err(|error| format!("cannot cancel session requests during disposal: {error}"))?
            .values()
        {
            cancel.store(true, Ordering::Release);
        }
        crate::tool_runtime::runtime_ops::announce_cancellation();
        self.inner
            .subscribers
            .lock()
            .map_err(|error| {
                format!("cannot clear session subscriptions during disposal: {error}")
            })?
            .clear();
        *self.inner.interactions.write().map_err(|error| {
            format!("cannot clear session interaction handler during disposal: {error}")
        })? = None;
        *self.inner.conversation.lock().map_err(|error| {
            format!("cannot release session conversation during disposal: {error}")
        })? = None;
        Ok(())
    }
}
