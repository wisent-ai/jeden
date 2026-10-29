//! Running one prompt for an editor session, and ending one.
//!
//! Split out of `rpc/acp/agent.rs`, which had grown past the module line cap.

use super::super::mapping::{map_session_event, prompt_text};
use super::interaction::AcpInteraction;
use super::{operation_expired, AcpState};
use crate::sdk::PromptRequest as JedenPromptRequest;
use crate::tool_runtime::runtime_ops::CancellationToken;
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Responder;
use futures::channel::oneshot;
use futures::executor::block_on;
use futures::future::{self, Either};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

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
        let session = crate::panicked::held(&self.sessions)
            .get(&session_id)
            .cloned()
            .ok_or_else(|| {
                agent_client_protocol::Error::resource_not_found(Some(session_id.clone()))
            })?;
        let subscription = session.subscribe().map_err(super::internal)?;
        let request_id = format!("acp-{}-{}", responder.id(), Uuid::new_v4());
        {
            let mut active = crate::panicked::held(&self.active);
            if active.contains_key(&session_id) {
                return Err(super::invalid_params(
                    "a prompt is already active for this session",
                ));
            }
            active.insert(session_id.clone(), request_id.clone());
        }

        let cancellation = responder.cancellation();
        let operation_token = CancellationToken::new();
        let input_supported = crate::panicked::held(&self.client_capabilities)
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
            let forward_session_id = session_id.clone();
            let forward_request_id = request_id.clone();
            let forward_client = client.clone();
            let subscription_id = subscription.id();

            // The peer's `$/cancel_request` is a future the protocol resolves;
            // this thread waits on it or on the prompt finishing, whichever
            // comes first, and turns a cancellation into the session abort.
            let (finished_tx, finished_rx) = oneshot::channel::<()>();
            let watch_session = session.clone();
            let watch_request_id = request_id.clone();
            let watch_cancellation = cancellation.clone();
            let watch_operation_token = operation_token.clone();
            let watcher = thread::spawn(move || {
                let cancelled = Box::pin(watch_cancellation.cancelled());
                if let Either::Left(_) = block_on(future::select(cancelled, finished_rx)) {
                    watch_operation_token.cancel();
                    if let Err(cause) = watch_session.abort(&watch_request_id) {
                        eprintln!("jeden acp: the cancelled request {watch_request_id} is not aborted: {cause}");
                    }
                }
            });

            // Blocks on the subscription; stops at this request's terminal
            // event or once the prompt has returned and the subscription is
            // ended below.
            let forwarder = thread::spawn(move || {
                let mut streamed = false;
                while let Ok(event) = subscription.recv() {
                    if event.request_id != forward_request_id {
                        continue;
                    }
                    let mapped = map_session_event(event.event, &mut streamed);
                    if let Some(update) = mapped.update {
                        // A client that hung up misses an update; the prompt's own
                        // answer still reaches it or fails on its own.
                        let _undelivered = forward_client.send_notification(
                            SessionNotification::new(forward_session_id.clone(), update),
                        );
                    }
                    if mapped.terminal {
                        break;
                    }
                }
            });

            let result = session.prompt(JedenPromptRequest {
                request_id: request_id.clone(),
                prompt,
                goal: None,
            });
            // Each cleanup step that fails is said: a subscription not ended or
            // a handler not reset outlives the prompt unnoticed. The active
            // prompt is cleared through a poisoned lock, so a panic elsewhere
            // does not leave the session refusing every later prompt.
            let _unreceived = finished_tx.send(()); // the watcher may already be gone
            if let Err(cause) = session.end_subscription(subscription_id) {
                eprintln!(
                    "jeden acp: the event subscription of {request_id} is not ended: {cause}"
                );
            }
            if watcher.join().is_err() {
                eprintln!("jeden acp: the cancellation watcher of {request_id} panicked");
            }
            if forwarder.join().is_err() {
                eprintln!("jeden acp: the event forwarder of {request_id} panicked");
            }
            if let Err(cause) = session.set_interaction_handler(None) {
                eprintln!("jeden acp: session {session_id} keeps its interaction handler: {cause}");
            }
            crate::panicked::held(&state.active).remove(&session_id);

            // The same entering rule as the active map above: each change is one
            // insert or remove, so a poisoned lock holds a consistent set.
            let aborted = crate::panicked::held(&state.aborted).remove(&request_id);
            let delivered =
                if cancellation.is_cancelled() || operation_expired(&operation_token) || aborted {
                    responder.respond(PromptResponse::new(StopReason::Cancelled))
                } else {
                    match result {
                        Ok(_) => responder.respond(PromptResponse::new(StopReason::EndTurn)),
                        Err(error) => responder.respond_with_error(super::internal(error)),
                    }
                };
            if let Err(error) = delivered {
                eprintln!("jeden acp: the answer to {request_id} could not be delivered: {error}");
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
        let session = crate::panicked::held(&self.sessions).get(&id).cloned();
        let request_id = crate::panicked::held(&self.active).get(&id).cloned();
        if let (Some(session), Some(request_id)) = (session, request_id) {
            crate::panicked::held(&self.aborted).insert(request_id.clone());
            // Whether a request was still running to abort; the aborted set
            // above already tells the prompt thread to answer Cancelled.
            let _was_running = session.abort(&request_id).map_err(super::internal)?;
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
        let session = crate::panicked::held(&self.sessions)
            .remove(&id)
            .ok_or_else(|| agent_client_protocol::Error::resource_not_found(Some(id.clone())))?;
        crate::panicked::held(&self.active).remove(&id);
        session
            .set_interaction_handler(None)
            .map_err(super::internal)?;
        session.dispose().map_err(super::internal)?;
        Ok(CloseSessionResponse::new())
    }
}
