use super::*;

impl<B: SessionBackend> HeadlessDaemon<B> {
    // `ErrorV1` is the wire envelope `jeden.session.v1` puts on the socket, so
    // an `Err` here is already the response body, not an internal error type.
    #[allow(clippy::result_large_err)]
    pub(super) fn dispatch(
        &self,
        connection: &AuthenticatedConnection,
        request: RequestEnvelopeV1,
    ) -> Result<Value, ErrorV1> {
        connection.validate_request(&request).map_err(|_| {
            protocol_error(
                "unsupported_protocol",
                "protocolVersion must be jeden.session.v1",
            )
        })?;
        if let Some(deadline) = request.meta.deadline_unix_millis {
            if now_unix_millis() >= deadline {
                return Err(protocol_error(
                    "deadline_exceeded",
                    "request deadline has elapsed",
                ));
            }
        }
        match request.method.as_str() {
            "health/readiness" | "readiness" => {
                Ok(json!({"state": format!("{:?}", self.service.readiness()).to_lowercase()}))
            }
            "session/create" => {
                let session_id = self
                    .service
                    .create_session(&connection.identity)
                    .map_err(service_error)?;
                let expires = now_unix().saturating_add(self.config.reconnect_ttl.as_secs());
                let reconnect_token = self
                    .reconnect
                    .issue(connection, &session_id, expires)
                    .map_err(|_| protocol_error("internal", "failed to issue reconnect token"))?;
                Ok(
                    json!({"sessionId": session_id, "reconnectToken": reconnect_token, "expiresUnix": expires}),
                )
            }
            "session/reconnect" => {
                let token = string_field(&request.params, "reconnectToken")?;
                let session_id = self
                    .reconnect
                    .verify(connection, token, now_unix())
                    .map_err(|_| {
                        protocol_error("access_denied", "invalid or expired reconnect token")
                    })?;
                Ok(json!({"sessionId": session_id}))
            }
            "session/prompt" | "session/completion/continue" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let outcome = if request.method == "session/completion/continue" {
                    self.service.continue_work(
                        &connection.identity,
                        session_id,
                        &request.meta.idempotency_key,
                    )
                } else {
                    let prompt = string_field(&request.params, "prompt")?;
                    self.service.submit_prompt(
                        &connection.identity,
                        session_id,
                        &request.meta.idempotency_key,
                        prompt,
                    )
                }
                .map_err(service_error)?;
                Ok(match outcome {
                    SubmitOutcome::Started { request_id } => {
                        json!({"state": "started", "requestId": request_id})
                    }
                    SubmitOutcome::Reattached { request_id } => {
                        json!({"state": "reattached", "requestId": request_id})
                    }
                    SubmitOutcome::Completed { request_id, result } => {
                        json!({"state": "completed", "requestId": request_id, "result": result})
                    }
                })
            }
            "session/replay" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let request_id = string_field(&request.params, "requestId")?;
                let cursor = request.params.get("cursor").and_then(Value::as_str);
                let limit = request
                    .params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .min(1000) as usize;
                let events = self
                    .service
                    .replay_from_token(&connection.identity, session_id, request_id, cursor, limit)
                    .map_err(service_error)?;
                Ok(json!({"events": events}))
            }
            "session/cancel" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let request_id = string_field(&request.params, "requestId")?;
                let cancelled = self
                    .service
                    .cancel(&connection.identity, session_id, request_id)
                    .map_err(service_error)?;
                Ok(json!({"cancelled": cancelled}))
            }
            "session/list" => {
                let limit = positive_limit(&request.params)?;
                let listing = self
                    .service
                    .list_sessions(&connection.identity, limit)
                    .map_err(service_error)?;
                Ok(json!({
                    "sessions": listing
                        .sessions
                        .iter()
                        .map(|session| session.wire_value())
                        .collect::<Vec<_>>(),
                    "skipped": listing.skipped,
                }))
            }
            "session/open" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let summary = self
                    .service
                    .open_session(&connection.identity, session_id)
                    .map_err(service_error)?;
                Ok(json!({
                    "sessionId": summary.session_id,
                    "path": summary.path,
                    "cwd": summary.cwd,
                    "turns": summary.turns,
                }))
            }
            "session/history" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let limit = positive_limit(&request.params)?;
                let (turns, truncated) = self
                    .service
                    .history(&connection.identity, session_id, limit)
                    .map_err(service_error)?;
                Ok(json!({"turns": turns, "truncated": truncated}))
            }
            "session/completion/get" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let completion = self
                    .service
                    .completion(&connection.identity, session_id)
                    .map_err(service_error)?;
                Ok(json!({"sessionId": session_id, "completion": completion}))
            }
            "session/completion/add" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let prompt = string_field(&request.params, "prompt")?;
                let completion = self
                    .service
                    .add_request(&connection.identity, session_id, prompt)
                    .map_err(service_error)?;
                Ok(json!({"sessionId": session_id, "completion": completion}))
            }
            "session/completion/control" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let task_id = string_field(&request.params, "taskId")?;
                let action = string_field(&request.params, "action")?;
                let reason = string_field(&request.params, "reason")?;
                let revision = request
                    .params
                    .get("revision")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        protocol_error("invalid_request", "revision must be an unsigned integer")
                    })?;
                let completion = self
                    .service
                    .control_completion(
                        &connection.identity,
                        session_id,
                        task_id,
                        action,
                        reason,
                        revision,
                    )
                    .map_err(service_error)?;
                Ok(json!({"sessionId": session_id, "completion": completion}))
            }
            _ => Err(protocol_error("method_not_found", "unknown session method")),
        }
    }
}
// `ErrorV1` is the wire envelope `jeden.session.v1` puts on the socket; boxing
// it here would change the shape every dispatch arm already returns.
#[allow(clippy::result_large_err)]
fn string_field<'a>(params: &'a Value, field: &str) -> Result<&'a str, ErrorV1> {
    params
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| protocol_error("invalid_request", &format!("missing {field}")))
}

/// An absent `limit` means "everything"; a present one must be a whole number
/// of at least one, so a `0` or a float is a named refusal, not a silent no-op.
// `ErrorV1` is the wire envelope `jeden.session.v1` puts on the socket; boxing
// it here would change the shape every dispatch arm already returns.
#[allow(clippy::result_large_err)]
fn positive_limit(params: &Value) -> Result<Option<usize>, ErrorV1> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(limit) if limit >= 1 => Ok(Some(usize::try_from(limit).unwrap_or(usize::MAX))),
            _ => Err(protocol_error(
                "invalid_request",
                "limit must be an integer of at least 1",
            )),
        },
    }
}

pub(super) fn wire_error(id: Value, error: ErrorV1) -> Value {
    json!({"id": id, "error": error})
}

fn protocol_error(code: &str, message: &str) -> ErrorV1 {
    ErrorV1 {
        code: code.into(),
        message: message.into(),
        retryable: false,
        details: json!({}),
    }
}

fn service_error(error: ServiceError) -> ErrorV1 {
    match error {
        ServiceError::AccessDenied => protocol_error("access_denied", "access denied"),
        ServiceError::InvalidRequest(message) => protocol_error("invalid_request", &message),
        ServiceError::Backpressure { retry_after_millis } => ErrorV1 {
            code: "backpressure".into(),
            message: "service capacity exhausted".into(),
            retryable: true,
            details: json!({"retryAfterMillis": retry_after_millis}),
        },
        ServiceError::NotReady => ErrorV1 {
            code: "not_ready".into(),
            message: "service is not ready".into(),
            retryable: true,
            details: json!({"retryAfterMillis": 100}),
        },
        ServiceError::Tenant(TenantError::QuotaExceeded { retry_after_millis }) => ErrorV1 {
            code: "quota_exceeded".into(),
            message: "tenant quota exceeded".into(),
            retryable: true,
            details: json!({"retryAfterMillis": retry_after_millis}),
        },
        ServiceError::Tenant(_) => protocol_error("access_denied", "access denied"),
        ServiceError::Idempotency(error) => {
            protocol_error("idempotency_error", &format!("{error:?}"))
        }
        ServiceError::Replay(error) => protocol_error("replay_error", &format!("{error:?}")),
        ServiceError::Runtime(message) => protocol_error("runtime_error", &message),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
