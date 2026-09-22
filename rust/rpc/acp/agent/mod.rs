//! The state one editor connection holds: which sessions exist, which are
//! running, and what the client said it can do.

use crate::sdk::{AgentSession, SessionOptions};
use crate::tool_runtime::runtime_ops::CancellationToken;
use agent_client_protocol::schema::{v1::*, ProtocolVersion};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

mod interaction;
mod prompt;
mod wiring;

use interaction::AcpInteraction;
pub(crate) use wiring::build_agent;
use crate::rpc::acp::agent::wiring::agent_capabilities;
use crate::rpc::acp::internal;
use crate::rpc::acp::invalid_params;

static NEXT_PROMPT: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(super) struct AcpState {
    pub(super) initialized: AtomicBool,
    pub(super) client_capabilities: Mutex<ClientCapabilities>,
    pub(super) sessions: Mutex<HashMap<String, AgentSession>>,
    pub(super) active: Mutex<HashMap<String, String>>,
}

impl AcpState {
    pub(super) fn initialize(
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
        .agent_info(Implementation::new("jeden", crate::JEDEN_VERSION)))
    }

    pub(super) fn require_initialized(&self) -> agent_client_protocol::Result<()> {
        if self.initialized.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(super::invalid_params(
                "initialize must complete before session methods",
            ))
        }
    }

    pub(super) fn new_session(
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

    pub(super) fn load_session(
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

pub(super) fn operation_expired(cancellation: &CancellationToken) -> bool {
    cancellation.is_cancelled()
}
