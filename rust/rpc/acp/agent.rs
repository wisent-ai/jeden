use super::mapping::{map_session_event, prompt_text};
use crate::sdk::{
    AgentSession, ApprovalRequest as JedenApprovalRequest, ElicitationRequest, InteractionHandler,
    PromptRequest as JedenPromptRequest, SessionOptions,
};
use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use agent_client_protocol::schema::{v1::*, ProtocolVersion};
use agent_client_protocol::{Agent, Client, ConnectionTo, Dispatch, Responder};
use futures::executor::block_on;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_PROMPT: AtomicU64 = AtomicU64::new(1);

pub(super) fn build_agent() -> impl agent_client_protocol::ConnectTo<Client> {
    let state = Arc::new(AcpState::default());
    Agent
        .builder()
        .name("jeden-acp")
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: InitializeRequest, responder, _cx| {
                    responder.respond_with_result(state.initialize(request))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: NewSessionRequest, responder, _cx| {
                    responder.respond_with_result(state.new_session(request))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: LoadSessionRequest, responder, _cx| {
                    responder.respond_with_result(state.load_session(request))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: PromptRequest,
                            responder: Responder<PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    state.start_prompt(request, responder, cx)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = Arc::clone(&state);
                async move |request: CloseSessionRequest, responder, _cx| {
                    responder.respond_with_result(state.close_session(&request.session_id))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = Arc::clone(&state);
                async move |notification: CancelNotification, _cx| {
                    state.cancel_session(&notification.session_id)
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_dispatch(
            async move |message: Dispatch, cx: ConnectionTo<Client>| {
                message.respond_with_error(agent_client_protocol::Error::method_not_found(), cx)
            },
            agent_client_protocol::on_receive_dispatch!(),
        )
}

fn agent_capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new())
        .session_capabilities(SessionCapabilities::new().close(SessionCloseCapabilities::new()))
}

#[derive(Default)]
struct AcpState {
    initialized: AtomicBool,
    client_capabilities: Mutex<ClientCapabilities>,
    sessions: Mutex<HashMap<String, AgentSession>>,
    active: Mutex<HashMap<String, String>>,
}

impl AcpState {
    fn initialize(
        &self,
        request: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        *self
            .client_capabilities
            .lock()
            .map_err(|_| super::internal("ACP capability lock poisoned"))? =
            request.client_capabilities;
        self.initialized.store(true, Ordering::Release);
        Ok(InitializeResponse::new(match request.protocol_version {
            ProtocolVersion::V1 => ProtocolVersion::V1,
            _ => ProtocolVersion::V1,
        })
        .agent_capabilities(agent_capabilities())
        .agent_info(Implementation::new("jeden", env!("CARGO_PKG_VERSION"))))
    }

    fn require_initialized(&self) -> agent_client_protocol::Result<()> {
        if self.initialized.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(super::invalid_params(
                "initialize must complete before session methods",
            ))
        }
    }

    fn new_session(
        &self,
        request: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        self.require_initialized()?;
        validate_workspace(
            &request.cwd,
            &request.additional_directories,
            request.mcp_servers.len(),
        )?;
        let session = AgentSession::new(SessionOptions {
            cwd: request.cwd,
            ..SessionOptions::default()
        })
        .map_err(super::internal)?;
        let session_id = session
            .session_path()
            .map_err(super::internal)?
            .display()
            .to_string();
        self.sessions
            .lock()
            .map_err(|_| super::internal("ACP session lock poisoned"))?
            .insert(session_id.clone(), session);
        Ok(NewSessionResponse::new(session_id))
    }

    fn load_session(
        &self,
        request: LoadSessionRequest,
    ) -> agent_client_protocol::Result<LoadSessionResponse> {
        self.require_initialized()?;
        validate_workspace(
            &request.cwd,
            &request.additional_directories,
            request.mcp_servers.len(),
        )?;
        let session_id = request.session_id.0.to_string();
        let session = AgentSession::resume(
            SessionOptions {
                cwd: request.cwd,
                ..SessionOptions::default()
            },
            &session_id,
        )
        .map_err(|error| {
            if error.contains("session not found") {
                agent_client_protocol::Error::resource_not_found(Some(session_id.clone()))
            } else {
                super::internal(error)
            }
        })?;
        self.sessions
            .lock()
            .map_err(|_| super::internal("ACP session lock poisoned"))?
            .insert(session_id, session);
        Ok(LoadSessionResponse::new())
    }

    fn start_prompt(
        self: &Arc<Self>,
        request: PromptRequest,
        responder: Responder<PromptResponse>,
        client: ConnectionTo<Client>,
    ) -> agent_client_protocol::Result<()> {
        self.require_initialized()?;
        let session_id = request.session_id.0.to_string();
        let prompt = prompt_text(request.prompt)?;
        let session = self
            .sessions
            .lock()
            .map_err(|_| super::internal("ACP session lock poisoned"))?
            .get(&session_id)
            .cloned()
            .ok_or_else(|| {
                agent_client_protocol::Error::resource_not_found(Some(session_id.clone()))
            })?;
        let subscription = session.subscribe().map_err(super::internal)?;
        let request_id = format!(
            "acp-{}-{}",
            responder.id(),
            NEXT_PROMPT.fetch_add(1, Ordering::Relaxed),
        );
        {
            let mut active = self
                .active
                .lock()
                .map_err(|_| super::internal("ACP active prompt lock poisoned"))?;
            if active.contains_key(&session_id) {
                return Err(super::invalid_params(
                    "a prompt is already active for this session",
                ));
            }
            active.insert(session_id.clone(), request_id.clone());
        }

        let cancellation = responder.cancellation();
        let operation_token = CancellationToken::new();
        let mut operation = OperationContext::new(
            operation_token.clone(),
            ArtifactSink::new(
                Path::new(&session_id)
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("acp-artifacts"),
            ),
        );
        if let Some(deadline) = deadline_from_meta(request.meta.as_ref()) {
            operation = operation.with_deadline(deadline);
        }
        let input_supported = self
            .client_capabilities
            .lock()
            .map_err(|_| super::internal("ACP capability lock poisoned"))?
            .elicitation
            .as_ref()
            .is_some_and(|capability| capability.form.is_some());
        let operation_deadline = operation.deadline();
        session
            .set_interaction_handler(Some(Arc::new(AcpInteraction {
                session_id: session_id.clone(),
                client: client.clone(),
                cancellation: operation_token.clone(),
                deadline: operation_deadline,
                input_supported,
            })))
            .map_err(super::internal)?;

        let state = Arc::clone(self);
        thread::spawn(move || {
            let forward_session = session.clone();
            let forward_session_id = session_id.clone();
            let forward_request_id = request_id.clone();
            let forward_client = client.clone();
            let forward_cancellation = cancellation.clone();
            let forward_operation_token = operation_token.clone();
            let forward_deadline = operation_deadline;
            let prompt_done = Arc::new(AtomicBool::new(false));
            let forward_done = Arc::clone(&prompt_done);
            let forwarder = thread::spawn(move || {
                let mut streamed = false;
                let mut cancellation_sent = false;
                loop {
                    if !cancellation_sent
                        && (forward_cancellation.is_cancelled()
                            || operation_expired(&forward_operation_token, forward_deadline))
                    {
                        forward_operation_token.cancel();
                        let _ = forward_session.abort(&forward_request_id);
                        cancellation_sent = true;
                    }
                    match subscription.recv_timeout(Duration::from_millis(50)) {
                        Ok(event) if event.request_id == forward_request_id => {
                            let mapped = map_session_event(event.event, &mut streamed);
                            if let Some(update) = mapped.update {
                                let _ = forward_client.send_notification(SessionNotification::new(
                                    forward_session_id.clone(),
                                    update,
                                ));
                            }
                            if mapped.terminal {
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                            if forward_done.load(Ordering::Acquire) =>
                        {
                            break
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });

            let result = session.prompt(JedenPromptRequest {
                request_id: request_id.clone(),
                prompt,
            });
            prompt_done.store(true, Ordering::Release);
            let _ = forwarder.join();
            let _ = session.set_interaction_handler(None);
            if let Ok(mut active) = state.active.lock() {
                active.remove(&session_id);
            }

            if cancellation.is_cancelled()
                || operation_expired(&operation_token, operation_deadline)
                || result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.to_ascii_lowercase().contains("cancel"))
            {
                let _ = responder.respond(PromptResponse::new(StopReason::Cancelled));
            } else {
                match result {
                    Ok(_) => {
                        let _ = responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                    Err(error) => {
                        let _ = responder.respond_with_error(super::internal(error));
                    }
                }
            }
        });
        Ok(())
    }

    fn cancel_session(&self, session_id: &SessionId) -> agent_client_protocol::Result<()> {
        self.require_initialized()?;
        let id = session_id.0.to_string();
        let session = self
            .sessions
            .lock()
            .map_err(|_| super::internal("ACP session lock poisoned"))?
            .get(&id)
            .cloned();
        let request_id = self
            .active
            .lock()
            .map_err(|_| super::internal("ACP active prompt lock poisoned"))?
            .get(&id)
            .cloned();
        if let (Some(session), Some(request_id)) = (session, request_id) {
            let _ = session.abort(&request_id).map_err(super::internal)?;
        }
        Ok(())
    }

    fn close_session(
        &self,
        session_id: &SessionId,
    ) -> agent_client_protocol::Result<CloseSessionResponse> {
        self.require_initialized()?;
        let id = session_id.0.to_string();
        self.cancel_session(session_id)?;
        let session = self
            .sessions
            .lock()
            .map_err(|_| super::internal("ACP session lock poisoned"))?
            .remove(&id)
            .ok_or_else(|| agent_client_protocol::Error::resource_not_found(Some(id.clone())))?;
        self.active
            .lock()
            .map_err(|_| super::internal("ACP active prompt lock poisoned"))?
            .remove(&id);
        session
            .set_interaction_handler(None)
            .map_err(super::internal)?;
        session.dispose().map_err(super::internal)?;
        Ok(CloseSessionResponse::new())
    }
}

impl Drop for AcpState {
    fn drop(&mut self) {
        let sessions = self
            .sessions
            .get_mut()
            .map(|sessions| {
                sessions
                    .drain()
                    .map(|(_, session)| session)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for session in sessions {
            let _ = session.set_interaction_handler(None);
            if let Ok(active) = session.status() {
                for request_id in active {
                    let _ = session.abort(&request_id);
                }
            }
            let _ = session.dispose();
        }
    }
}

fn validate_workspace(
    cwd: &Path,
    additional: &[std::path::PathBuf],
    mcp_count: usize,
) -> agent_client_protocol::Result<()> {
    if !cwd.is_absolute() {
        return Err(super::invalid_params("cwd must be absolute"));
    }
    if !additional.is_empty() {
        return Err(super::unsupported(
            "session.additionalDirectories",
            "additionalDirectories are not supported",
        ));
    }
    if mcp_count != 0 {
        return Err(super::unsupported(
            "session.mcpServers",
            "mcpServers are not supported by this adapter",
        ));
    }
    Ok(())
}

fn deadline_from_meta(meta: Option<&Meta>) -> Option<Instant> {
    let meta = meta?;
    let millis = meta
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .or_else(|| meta.get("deadlineMs").and_then(Value::as_u64))?;
    Some(Instant::now() + Duration::from_millis(millis))
}

fn operation_expired(cancellation: &CancellationToken, deadline: Option<Instant>) -> bool {
    cancellation.is_cancelled() || deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

struct AcpInteraction {
    session_id: String,
    client: ConnectionTo<Client>,
    cancellation: CancellationToken,
    deadline: Option<Instant>,
    input_supported: bool,
}

impl AcpInteraction {
    fn ready(&self) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("ACP interaction cancelled".into());
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err("ACP interaction deadline exceeded".into());
        }
        Ok(())
    }
}

impl InteractionHandler for AcpInteraction {
    fn elicit(&self, request: ElicitationRequest) -> Result<String, String> {
        self.ready()?;
        if !self.input_supported {
            return Err("unsupported ACP client capability: elicitation.form".into());
        }
        let mut property = StringPropertySchema::new().title(request.question.clone());
        if !request.options.is_empty() {
            property = property.enum_values(request.options.clone());
        }
        let schema = ElicitationSchema::new().property("answer", property, true);
        let mode = ElicitationFormMode::new(
            ElicitationSessionScope::new(self.session_id.clone()),
            schema,
        );
        let response = block_on(
            self.client
                .send_request(CreateElicitationRequest::new(mode, request.question))
                .block_task(),
        )
        .map_err(|error| error.to_string())?;
        self.ready()?;
        match response.action {
            ElicitationAction::Accept(accepted) => {
                let content = accepted
                    .content
                    .ok_or_else(|| "ACP elicitation accepted without content".to_string())?;
                let answer = content
                    .get("answer")
                    .ok_or_else(|| "ACP elicitation response omitted answer".to_string())?;
                serde_json::to_value(answer)
                    .map_err(|error| error.to_string())?
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "ACP elicitation answer must be a string".to_string())
            }
            ElicitationAction::Decline => Err("ACP elicitation declined".into()),
            ElicitationAction::Cancel => Err("ACP elicitation cancelled".into()),
            _ => Err("unsupported ACP elicitation response action".into()),
        }
    }

    fn approve(&self, request: JedenApprovalRequest) -> Result<bool, String> {
        self.ready()?;
        let tool_call = ToolCallUpdate::new(
            request.token,
            ToolCallUpdateFields::new()
                .title(request.tool)
                .status(ToolCallStatus::Pending)
                .raw_input(json!({"detail": request.detail})),
        );
        let options = vec![
            PermissionOption::new("allow-once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("reject-once", "Reject", PermissionOptionKind::RejectOnce),
        ];
        let response = block_on(
            self.client
                .send_request(RequestPermissionRequest::new(
                    self.session_id.clone(),
                    tool_call,
                    options,
                ))
                .block_task(),
        )
        .map_err(|error| error.to_string())?;
        self.ready()?;
        Ok(matches!(
            response.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == "allow-once"
        ))
    }
}
