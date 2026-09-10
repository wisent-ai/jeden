use super::*;

impl<B: SessionBackend> SessionService<B> {
    pub fn submit_prompt(
        self: &Arc<Self>,
        caller: &TenantPrincipal,
        session_id: &str,
        idempotency_key: &str,
        prompt: &str,
    ) -> Result<SubmitOutcome, ServiceError> {
        self.submit_work(caller, session_id, idempotency_key, prompt, false)
    }

    pub fn continue_work(
        self: &Arc<Self>,
        caller: &TenantPrincipal,
        session_id: &str,
        idempotency_key: &str,
    ) -> Result<SubmitOutcome, ServiceError> {
        self.submit_work(
            caller,
            session_id,
            idempotency_key,
            "Continue retained work",
            true,
        )
    }

    pub fn completion(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<Value, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.backend
            .completion(&caller.tenant, session_id)
            .map_err(ServiceError::Runtime)
    }

    pub fn add_request(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        prompt: &str,
    ) -> Result<Value, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.backend
            .add_request(&caller.tenant, session_id, prompt)
            .map_err(ServiceError::Runtime)
    }

    pub fn control_completion(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        task_id: &str,
        action: &str,
        reason: &str,
        revision: u64,
    ) -> Result<Value, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.backend
            .control_completion(
                &caller.tenant,
                session_id,
                task_id,
                action,
                reason,
                revision,
            )
            .map_err(ServiceError::Runtime)
    }

    fn submit_work(
        self: &Arc<Self>,
        caller: &TenantPrincipal,
        session_id: &str,
        idempotency_key: &str,
        prompt: &str,
        continuing: bool,
    ) -> Result<SubmitOutcome, ServiceError> {
        if prompt.trim().is_empty() {
            return Err(ServiceError::InvalidRequest(
                "prompt must not be empty".into(),
            ));
        }
        self.authorize_session(caller, session_id)?;
        let request_digest = IdempotencyStore::request_digest(
            json!({"sessionId": session_id, "prompt": prompt, "continuing": continuing})
                .to_string()
                .as_bytes(),
        );
        let request_id = format!(
            "request-{}-{}",
            self.instance,
            self.next_request.fetch_add(1, Ordering::Relaxed)
        );
        match self.idempotency.begin(
            &caller.tenant,
            idempotency_key,
            &request_digest,
            &request_id,
        )? {
            IdempotencyDecision::Reattach { request_id } => {
                return Ok(SubmitOutcome::Reattached { request_id })
            }
            IdempotencyDecision::Completed { request_id, result } => {
                return Ok(SubmitOutcome::Completed { request_id, result })
            }
            IdempotencyDecision::Start => {}
        }
        let permit = match self.tenants.reserve_request(&caller.tenant) {
            Ok(permit) => permit,
            Err(error) => {
                self.idempotency.abandon(
                    &caller.tenant,
                    idempotency_key,
                    &request_digest,
                    &request_id,
                )?;
                return Err(error.into());
            }
        };
        let service = self.clone();
        let tenant = caller.tenant.clone();
        let session_id = session_id.to_owned();
        let key = idempotency_key.to_owned();
        let prompt = prompt.to_owned();
        let returned_request_id = request_id.clone();
        let rollback_tenant = tenant.clone();
        let rollback_key = key.clone();
        let rollback_digest = request_digest.clone();
        let rollback_request = request_id.clone();
        let submission = self.executor.submit(move || {
            let _permit = permit;
            let stream_id = request_id.clone();
            let replay = service.replay.clone();
            let event_tenant = tenant.clone();
            let event_session = session_id.clone();
            let event_request = request_id.clone();
            let emit = Arc::new(move |kind: String, payload: Value, terminal: bool| {
                let _ = replay.append(
                    &event_tenant,
                    SessionEventV1 {
                        session_id: event_session.clone(),
                        stream_id: stream_id.clone(),
                        sequence: 0,
                        event_id: String::new(),
                        request_id: event_request.clone(),
                        kind,
                        payload,
                        terminal,
                    },
                );
            });
            let result = service.backend.prompt(
                &tenant,
                &session_id,
                &request_id,
                &prompt,
                continuing,
                emit.clone(),
            );
            let cached = match result {
                Ok(value) => value,
                Err(message) => json!({"error": {"code": "runtime_error", "message": message}}),
            };
            let _ = service
                .idempotency
                .complete(&tenant, &key, &request_digest, cached);
        });
        if let Err(error) = submission {
            self.idempotency.abandon(
                &rollback_tenant,
                &rollback_key,
                &rollback_digest,
                &rollback_request,
            )?;
            return Err(match error {
                SubmitError::NotReady => ServiceError::NotReady,
                SubmitError::Backpressure { retry_after_millis } => {
                    ServiceError::Backpressure { retry_after_millis }
                }
            });
        }
        Ok(SubmitOutcome::Started {
            request_id: returned_request_id,
        })
    }

    pub fn replay(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        request_id: &str,
        after: EventCursor,
        limit: usize,
    ) -> Result<Vec<SessionEventV1>, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.replay
            .replay(&caller.tenant, session_id, request_id, after, limit)
            .map_err(Into::into)
    }

    pub fn replay_from_token(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        request_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, ServiceError> {
        let cursor = match cursor {
            Some(token) => EventCursor::parse(token)?,
            None => EventCursor(0),
        };
        self.replay(caller, session_id, request_id, cursor, limit)
            .map(|events| events.into_iter().map(|event| event.wire_value()).collect())
    }

    pub fn cancel(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        request_id: &str,
    ) -> Result<bool, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.backend
            .abort(&caller.tenant, session_id, request_id)
            .map_err(ServiceError::Runtime)
    }

    fn authorize_session(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<(), ServiceError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?;
        // An opened host session is registered in the same map under the same
        // host session id, so it authorizes exactly like a created one.
        match sessions.get(session_id) {
            Some(entry) if entry.tenant == caller.tenant => Ok(()),
            _ => Err(ServiceError::AccessDenied),
        }
    }

    pub fn artifact_path(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        relative: &Path,
    ) -> Result<PathBuf, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.tenants
            .scoped_path(
                &caller.tenant,
                &PathBuf::from("artifacts").join(session_id).join(relative),
            )
            .map_err(Into::into)
    }

    pub fn readiness(&self) -> Readiness {
        self.executor.readiness()
    }

    pub fn drain(&self, timeout: Duration) -> Result<(), ServiceError> {
        self.executor.drain(timeout).map_err(ServiceError::Runtime)
    }
}
