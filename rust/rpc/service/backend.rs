use super::*;
impl SessionBackend for AgentSessionFacade {
    fn create(&self, tenant: &TenantId, session_id: &str) -> Result<PathBuf, String> {
        let cwd = self
            .tenant_guard
            .tenant_root(tenant)
            .join("workspaces")
            .join(session_id);
        std::fs::create_dir_all(&cwd).map_err(|error| error.to_string())?;
        let session = AgentSession::new(SessionOptions::at(cwd))?;
        let path = session.session_path()?;
        crate::panicked::held(&self.sessions)
            .insert((tenant.as_str().to_owned(), session_id.to_owned()), session);
        Ok(path)
    }

    fn open(&self, tenant: &TenantId, session_id: &str, dir: &Path) -> Result<PathBuf, String> {
        // `AgentSession::resume_in_place` replays the ledger at `dir` and keeps
        // its recorder pointed at that same directory, so every later turn
        // appends to the ledger the operator's own terminal wrote. Plain
        // `AgentSession::resume` would seed a fresh session directory instead,
        // which is a fork, not a continuation. The session's own recorded `cwd`
        // is reused as the working directory so tools resolve exactly as they
        // did on the host.
        let session = AgentSession::resume_in_place(SessionOptions::at(session_cwd(dir)?), dir)?;
        let path = session.session_path()?;
        crate::panicked::held(&self.sessions)
            .insert((tenant.as_str().to_owned(), session_id.to_owned()), session);
        Ok(path)
    }

    fn turns(&self, dir: &Path) -> Result<Vec<Value>, String> {
        crate::cli::sessions::session_conversation_turns(dir)
    }

    fn prompt(
        &self,
        tenant: &TenantId,
        session_id: &str,
        request_id: &str,
        prompt: &str,
        continuing: bool,
        emit: Arc<dyn Fn(String, Value, bool) + Send + Sync>,
    ) -> Result<Value, String> {
        let session = self.session(tenant, session_id)?;
        let subscription = session.subscribe()?;
        let subscription_id = subscription.id();
        let forwarding_request = request_id.to_owned();
        let forward_emit = emit.clone();
        // Blocks on the subscription; stops at this request's terminal event or
        // once the prompt has returned and the subscription is ended below.
        let forwarder = thread::spawn(move || {
            while let Ok(event) = subscription.recv() {
                if event.request_id != forwarding_request {
                    continue;
                }
                let (kind, payload, terminal) = map_event(event.event);
                forward_emit(kind, payload, terminal);
                if terminal {
                    break;
                }
            }
        });
        let result = if continuing {
            session.continue_work(request_id.to_owned())
        } else {
            session.prompt(PromptRequest {
                request_id: request_id.to_owned(),
                prompt: prompt.to_owned(),
                goal: None,
            })
        };
        session.end_subscription(subscription_id)?;
        if let Err(error) = &result {
            emit("error".into(), json!({"message": error}), true);
        }
        forwarder
            .join()
            .map_err(|payload| crate::panicked::refusal("the session event forwarder", &*payload))?;
        let result = result?;
        serde_json::to_value(result).map_err(|error| {
            format!("request {request_id} finished, but its result cannot be written as JSON: {error}")
        })
    }

    fn abort(&self, tenant: &TenantId, session_id: &str, request_id: &str) -> Result<bool, String> {
        self.session(tenant, session_id)?.abort(request_id)
    }

    fn completion(&self, tenant: &TenantId, session_id: &str) -> Result<Value, String> {
        self.session(tenant, session_id)?.completion()
    }

    fn add_request(
        &self,
        tenant: &TenantId,
        session_id: &str,
        prompt: &str,
    ) -> Result<Value, String> {
        self.session(tenant, session_id)?.add_request(prompt)
    }

    fn control_completion(
        &self,
        tenant: &TenantId,
        session_id: &str,
        task_id: &str,
        action: &str,
        reason: &str,
        revision: u64,
    ) -> Result<Value, String> {
        self.session(tenant, session_id)?
            .control_completion(task_id, action, reason, revision)
    }
}
