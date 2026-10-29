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
            crate::tally::draw(&self.next_request)
        );
        match self.idempotency.begin(
            &caller.tenant,
            idempotency_key,
            &request_digest,
            &request_id,
        )? {
            IdempotencyDecision::Reattach { request_id } => {
                // A request that finished without its result recorded is
                // answered with the result this daemon kept, not reattached
                // to a stream that already ended.
                let kept = crate::panicked::held(&self.unrecorded)
                    .get(&(
                        caller.tenant.as_str().to_owned(),
                        idempotency_key.to_owned(),
                    ))
                    .cloned();
                return Ok(match kept {
                    Some((request_id, result)) => SubmitOutcome::Completed { request_id, result },
                    None => SubmitOutcome::Reattached { request_id },
                });
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
            // An event the replay journal refuses is lost to every client
            // reading the stream, so the refusal is said with its event.
            let emit = Arc::new(move |kind: String, payload: Value, terminal: bool| {
                let described = kind.clone();
                if let Err(error) = replay.append(
                    &event_tenant,
                    EventDraft {
                        session_id: event_session.clone(),
                        stream_id: stream_id.clone(),
                        request_id: event_request.clone(),
                        kind,
                        payload,
                        terminal,
                    },
                ) {
                    eprintln!(
                        "jeden rpc: the {described} event of request {event_request} in session {event_session} could not be recorded for replay: {error:?}"
                    );
                }
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
            if let Err(error) = service
                .idempotency
                .complete(&tenant, &key, &request_digest, cached.clone())
            {
                eprintln!(
                    "jeden rpc: request {request_id} in session {session_id} finished but its result could not be recorded: {error:?}; a retry with the same key is answered from memory until this daemon stops"
                );
                crate::panicked::held(&service.unrecorded).insert(
                    (tenant.as_str().to_owned(), key.clone()),
                    (request_id.clone(), cached),
                );
            }
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
                SubmitError::Backpressure => ServiceError::Backpressure,
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
        limit: Option<usize>,
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
        limit: Option<usize>,
    ) -> Result<Vec<Value>, ServiceError> {
        let cursor = match cursor {
            Some(token) => EventCursor::parse(token)?,
            None => EventCursor::default(),
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
        let sessions = crate::panicked::held(&self.sessions);
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

    pub fn drain(&self) -> Result<(), ServiceError> {
        self.executor.drain().map_err(ServiceError::Runtime)
    }
}
