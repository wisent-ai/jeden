mod daemon;
mod idempotency;
mod replay;
mod service;
mod tenant;
mod tls;
mod transport;

pub use daemon::{BoundedExecutor, HeadlessConfig, HeadlessDaemon, Readiness, SubmitError};
pub use idempotency::{IdempotencyDecision, IdempotencyError, IdempotencyStore};
pub use replay::{EventCursor, ReplayError, ReplayStore, SessionEventV1};
pub use service::{
    AgentSessionFacade, ServiceError, SessionBackend, SessionListing, SessionService,
    SessionSummary, SubmitOutcome,
};
pub use tenant::{
    TenantDirectory, TenantError, TenantGuard, TenantId, TenantLimits, TenantPrincipal,
};
pub use tls::{
    ClientCertificateVerifier, MtlsConfig, PeerCertificate, ReloadableTlsAcceptor,
    ReloadableTrustStore, TlsError, TlsHandshake, TlsVersion, VerifiedPeer, REQUIRED_ALPN,
};
pub use transport::{
    AuthenticatedConnection, ErrorV1, ReconnectTokens, RequestEnvelopeV1, RequestMetaV1,
    TransportError, PROTOCOL_VERSION,
};

mod acp;
mod interaction;
mod server;

pub use acp::serve_stdio as serve_acp_stdio;
pub use server::{serve, serve_headless_cli, serve_stdio};
