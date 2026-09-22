//! Which requests this agent answers, and what it declares it can do.
//!
//! Split out of `rpc/acp/agent.rs`, which had grown past the module line cap.

use super::AcpState;
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::{Agent, Client, Dispatch, Responder};
use serde_json::json;
use std::sync::Arc;
use agent_client_protocol::ConnectionTo;

pub(crate) fn build_agent() -> impl agent_client_protocol::ConnectTo<Client> {
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

pub(crate) fn agent_capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new())
        .session_capabilities(SessionCapabilities::new().close(SessionCloseCapabilities::new()))
}
