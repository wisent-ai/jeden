//! Running one prompt for an editor session, and ending one.
//!
//! Split out of `rpc/acp/agent.rs`, which had grown past the module line cap.

use super::super::mapping::{map_session_event, prompt_text};
use super::interaction::AcpInteraction;
use super::{operation_expired, AcpState, NEXT_PROMPT};
use crate::sdk::PromptRequest as JedenPromptRequest;
use crate::tool_runtime::runtime_ops::CancellationToken;
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Responder;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

impl AcpState {
    pub(super) fn start_prompt(
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
        let input_supported = self
            .client_capabilities
            .lock()
            .map_err(|_| super::internal("ACP capability lock poisoned"))?
            .elicitation
            .as_ref()
            .is_some_and(|capability| capability.form.is_some());

        session
            .set_interaction_handler(Some(Arc::new(AcpInteraction {
                session_id: session_id.clone(),
                client: client.clone(),
                cancellation: operation_token.clone(),

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

            let prompt_done = Arc::new(AtomicBool::new(false));
            let forward_done = Arc::clone(&prompt_done);
            let forwarder = thread::spawn(move || {
                let mut streamed = false;
                let mut cancellation_sent = false;
                loop {
                    if !cancellation_sent
                        && (forward_cancellation.is_cancelled()
                            || operation_expired(&forward_operation_token))
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
                goal: None,
            });
            prompt_done.store(true, Ordering::Release);
            let _ = forwarder.join();
            let _ = session.set_interaction_handler(None);
            if let Ok(mut active) = state.active.lock() {
                active.remove(&session_id);
            }

            if cancellation.is_cancelled()
                || operation_expired(&operation_token)
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

    pub(super) fn cancel_session(
        &self,
        session_id: &SessionId,
    ) -> agent_client_protocol::Result<()> {
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

    pub(super) fn close_session(
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
