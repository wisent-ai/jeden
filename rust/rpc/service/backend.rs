use super::*;
impl SessionBackend for AgentSessionFacade {
    fn create(&self, tenant: &TenantId, session_id: &str) -> Result<PathBuf, String> {
        let cwd = self
            .tenant_guard
            .tenant_root(tenant)
            .join("workspaces")
            .join(session_id);
        std::fs::create_dir_all(&cwd).map_err(|error| error.to_string())?;
        let session = AgentSession::new(SessionOptions {
            cwd,
            ..SessionOptions::default()
        })?;
        let path = session.session_path()?;
        self.sessions
            .lock()
            .map_err(|_| "agent session lock poisoned".to_string())?
            .insert((tenant.as_str().to_owned(), session_id.to_owned()), session);
        Ok(path)
    }

    fn open(&self, tenant: &TenantId, session_id: &str, dir: &Path) -> Result<PathBuf, String> {
        // `AgentSession::resume_in_place` (rust/sdk/session.rs:135) replays the
        // ledger at `dir` and keeps its recorder pointed at that same directory,
        // so every later turn appends to the ledger the operator's own terminal
        // wrote. Plain `AgentSession::resume` (rust/sdk/session.rs:112) would
        // seed a fresh session directory instead, which is a fork, not a
        // continuation. The session's own recorded `cwd` is reused as the
        // working directory so tools resolve exactly as they did on the host.
        let session = AgentSession::resume_in_place(
            SessionOptions {
                cwd: session_cwd(dir)?,
                ..SessionOptions::default()
            },
            dir,
        )?;
        let path = session.session_path()?;
        self.sessions
            .lock()
            .map_err(|_| "agent session lock poisoned".to_string())?
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
        let forwarding_request = request_id.to_owned();
        let forward_emit = emit.clone();
        let forwarder = thread::spawn(move || loop {
            match subscription.recv_timeout(Duration::from_millis(250)) {
                Ok(event) if event.request_id == forwarding_request => {
                    let (kind, payload, terminal) = map_event(event.event);
                    forward_emit(kind, payload, terminal);
                    if terminal {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
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
        if let Err(error) = &result {
            emit("error".into(), json!({"message": error}), true);
        }
        forwarder
            .join()
            .map_err(|_| "session event forwarder panicked".to_string())?;
        result.map(|result| {
            serde_json::to_value(result).unwrap_or_else(|_| json!({"requestId": request_id}))
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
