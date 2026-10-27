//! The state one editor connection holds: which sessions exist, which are
//! running, and what the client said it can do.

use crate::sdk::{AgentSession, SessionOptions};
use crate::tool_runtime::runtime_ops::CancellationToken;
use agent_client_protocol::schema::{v1::*, ProtocolVersion};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

mod interaction;
mod prompt;
mod wiring;

use crate::rpc::acp::agent::wiring::agent_capabilities;
use crate::rpc::acp::internal;
use crate::rpc::acp::invalid_params;
pub(crate) use wiring::build_agent;

#[derive(Default)]
pub(super) struct AcpState {
    pub(super) initialized: AtomicBool,
    pub(super) client_capabilities: Mutex<ClientCapabilities>,
    pub(super) sessions: Mutex<HashMap<String, AgentSession>>,
    pub(super) active: Mutex<HashMap<String, String>>,
    /// Prompt request ids a `session/cancel` aborted, so the prompt thread
    /// answers Cancelled from this record rather than from its error's words.
    pub(super) aborted: Mutex<HashSet<String>>,
}

impl AcpState {
    pub(super) fn initialize(
        &self,
        request: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        // Every ACP lock holds a value changed by whole steps (a replace, an
        // insert, a remove), so a holder that panicked is recovered from.
        *crate::panicked::held(&self.client_capabilities) = request.client_capabilities;
        self.initialized.store(true, Ordering::Release);
        // This agent speaks one protocol version and answers it whatever the
        // client asked for; the client decides whether it can speak it too.
        Ok(InitializeResponse::new(ProtocolVersion::V1)
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
            !request.mcp_servers.is_empty(),
        )?;
        let session =
            AgentSession::new(SessionOptions::at(request.cwd)).map_err(super::internal)?;
        let session_id = session
            .session_path()
            .map_err(super::internal)?
            .display()
            .to_string();
        crate::panicked::held(&self.sessions).insert(session_id.clone(), session);
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
            !request.mcp_servers.is_empty(),
        )?;
        let session_id = request.session_id.0.to_string();
        if !AgentSession::exists(&session_id).map_err(super::internal)? {
            return Err(agent_client_protocol::Error::resource_not_found(Some(
                session_id,
            )));
        }
        let session = AgentSession::resume(SessionOptions::at(request.cwd), &session_id)
            .map_err(super::internal)?;
        crate::panicked::held(&self.sessions).insert(session_id, session);
        Ok(LoadSessionResponse::new())
    }
}

impl Drop for AcpState {
    // Every session this client opened is stopped and disposed, even after a
    // panic poisoned the map: left out, its turns would keep running for a
    // client that is gone. A step that fails is said with the session's cause.
    fn drop(&mut self) {
        let sessions = match self.sessions.get_mut() {
            Ok(sessions) => sessions,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (id, session) in sessions.drain() {
            if let Err(cause) = session.set_interaction_handler(None) {
                eprintln!("jeden acp: session {id} keeps its interaction handler: {cause}");
            }
            match session.status() {
                Ok(active) => {
                    for request_id in active {
                        if let Err(cause) = session.abort(&request_id) {
                            eprintln!("jeden acp: request {request_id} of session {id} is not aborted: {cause}");
                        }
                    }
                }
                Err(cause) => eprintln!(
                    "jeden acp: the running requests of session {id} cannot be read: {cause}"
                ),
            }
            if let Err(cause) = session.dispose() {
                eprintln!("jeden acp: session {id} is not disposed: {cause}");
            }
        }
    }
}

fn validate_workspace(
    cwd: &Path,
    additional: &[std::path::PathBuf],
    names_mcp_servers: bool,
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
    if names_mcp_servers {
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
