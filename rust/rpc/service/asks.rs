use super::*;

impl<B: SessionBackend> SessionService<B> {
    /// The register asks this caller may see: those first asked in a
    /// workspace it is granted, or asked in a session it holds.
    pub fn asks(&self, caller: &TenantPrincipal) -> Result<Value, ServiceError> {
        let held = self.held_ask_sessions(caller)?;
        let mut value = crate::completion::list_asks().map_err(ServiceError::Runtime)?;
        if let Some(object) = value.as_object_mut() {
            // The register's host path is this machine's, not the caller's.
            object.remove("register");
        }
        if let Some(asks) = value.get_mut("asks").and_then(Value::as_array_mut) {
            asks.retain(|ask| visible(caller, ask, &held));
        }
        Ok(value)
    }

    /// One ask with every place it was asked; an ask the caller may not see
    /// is refused like one that does not exist.
    pub fn ask(&self, caller: &TenantPrincipal, ask_id: &str) -> Result<Value, ServiceError> {
        let held = self.held_ask_sessions(caller)?;
        let value = crate::completion::show_ask(ask_id).map_err(ServiceError::Runtime)?;
        if !visible(caller, &value, &held) {
            return Err(ServiceError::AccessDenied);
        }
        Ok(value)
    }

    pub fn answer_ask(
        &self,
        caller: &TenantPrincipal,
        ask_id: &str,
        text: &str,
    ) -> Result<Value, ServiceError> {
        self.ask(caller, ask_id)?;
        crate::completion::answer_ask(ask_id, text).map_err(ServiceError::Runtime)
    }

    fn held_ask_sessions(&self, caller: &TenantPrincipal) -> Result<Vec<String>, ServiceError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?;
        Ok(sessions
            .values()
            .filter(|entry| entry.tenant == caller.tenant)
            .map(|entry| crate::completion::ask_session_key(&entry.path))
            .collect())
    }
}

fn visible(caller: &TenantPrincipal, ask: &Value, held: &[String]) -> bool {
    ask["workspace"]
        .as_str()
        .is_some_and(|workspace| caller.grants_path(Path::new(workspace)))
        || ask["sessions"].as_array().is_some_and(|sessions| {
            sessions
                .iter()
                .filter_map(Value::as_str)
                .any(|session| held.iter().any(|path| path == session))
        })
}
