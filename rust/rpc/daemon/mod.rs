mod connection;
mod dispatch;
mod executor;
mod member;
use connection::reject_overloaded;
use dispatch::*;
pub use executor::{BoundedExecutor, Readiness, SubmitError};

use super::identity::{IdentityRefusal, WisentIdentityAuthority};
use super::service::{ServiceError, SessionBackend, SessionService, SubmitOutcome};
use super::tenant::{TenantDirectory, TenantError};
use super::tls::{ReloadableTlsAcceptor, TlsPeer};
use super::transport::{AuthenticatedConnection, ErrorV1, ReconnectTokens, RequestEnvelopeV1};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;

use std::sync::Arc;
use std::time::Duration;

/// The bounds a headless daemon keeps, each named by the host's declaration;
/// none is compiled in. Built only through [`HeadlessConfig::new`], so a
/// frame or connection bound is never zero.
#[derive(Debug, Clone)]
pub struct HeadlessConfig {
    pub(in crate::rpc::daemon) max_frame_bytes: usize,
    pub(in crate::rpc::daemon) max_connections: usize,
    pub(in crate::rpc::daemon) reconnect_key: Vec<u8>,
    pub(in crate::rpc::daemon) reconnect_ttl: Duration,
}

impl HeadlessConfig {
    pub fn new(
        max_frame_bytes: NonZeroUsize,
        max_connections: NonZeroUsize,
        reconnect_key: Vec<u8>,
        reconnect_ttl: Duration,
    ) -> Self {
        Self {
            max_frame_bytes: max_frame_bytes.get(),
            max_connections: max_connections.get(),
            reconnect_key,
            reconnect_ttl,
        }
    }
}

pub struct HeadlessDaemon<B: SessionBackend> {
    tls: ReloadableTlsAcceptor,
    directory: TenantDirectory,
    service: Arc<SessionService<B>>,
    config: HeadlessConfig,
    reconnect: ReconnectTokens,
    identity: Option<WisentIdentityAuthority>,
}

impl<B: SessionBackend> HeadlessDaemon<B> {
    pub fn new(
        tls: ReloadableTlsAcceptor,
        directory: TenantDirectory,
        service: Arc<SessionService<B>>,
        config: HeadlessConfig,
    ) -> Result<Self, String> {
        let reconnect = ReconnectTokens::new(config.reconnect_key.clone())
            .map_err(|error| format!("the reconnect key cannot sign reconnect tokens: {error:?}"))?;
        Ok(Self {
            tls,
            directory,
            service,
            config,
            reconnect,
            identity: None,
        })
    }

    /// Admit connections without a client certificate that authenticate with
    /// a Wisent identity of an organization the directory maps.
    pub fn with_wisent_identity(mut self, authority: WisentIdentityAuthority) -> Self {
        self.identity = Some(authority);
        self
    }

    pub fn local_addr(listener: &TcpListener) -> Result<SocketAddr, String> {
        listener.local_addr().map_err(|error| error.to_string())
    }

    pub async fn serve(
        self: Arc<Self>,
        listener: TcpListener,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        let admission = Arc::new(Semaphore::new(self.config.max_connections));
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|error| format!("TCP accept failed: {error}"))?;
                    let daemon = self.clone();
                    let permit = admission.clone().try_acquire_owned();
                    connections.spawn(async move {
                        let _permit = match permit {
                            Ok(permit) => permit,
                            // Every admitted connection holds a permit: none
                            // is left, so this one is answered as overloaded.
                            Err(tokio::sync::TryAcquireError::NoPermits | tokio::sync::TryAcquireError::Closed) => {
                                reject_overloaded(stream).await;
                                return;
                            }
                        };
                        daemon.serve_said(stream).await;
                    });
                }
            }
        }
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.drain())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("graceful drain failed: {error:?}"))?;
        while let Some(joined) = connections.join_next().await {
            if let Err(error) = joined {
                eprintln!("jeden daemon: a connection task did not finish: {error}");
            }
        }
        Ok(())
    }
}
