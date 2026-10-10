use super::*;

impl AgentSession {
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
