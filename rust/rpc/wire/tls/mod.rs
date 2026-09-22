//! Mutual TLS on the session port: who is allowed to connect, and the trust
//! state that can be reloaded without dropping the listener.

use rustls::ServerConfig;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

mod loading;

use loading::{load_server_config, peer_from_der};

pub const REQUIRED_ALPN: &str = "jeden.session.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVersion {
    V1_2,
    V1_3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCertificate {
    pub serial: String,
    pub issuer_fingerprint: String,
    pub dns_sans: Vec<String>,
    pub uri_sans: Vec<String>,
    pub not_before_unix: u64,
    pub not_after_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsHandshake {
    pub version: TlsVersion,
    pub alpn: String,
    pub peer_chain: Vec<PeerCertificate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPeer {
    pub certificate: PeerCertificate,
    pub trust_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsError {
    VersionRequired,
    ProtocolRequired,
    ClientCertificateRequired,
    UntrustedIssuer,
    NotYetValid,
    Expired,
    Revoked,
    MissingIdentitySan,
    TrustStateUnavailable,
}

pub trait ClientCertificateVerifier: Send + Sync {
    fn verify(&self, handshake: &TlsHandshake, now_unix: u64) -> Result<VerifiedPeer, TlsError>;
}

#[derive(Debug, Clone)]
struct TrustState {
    pub(super) generation: u64,
    trusted_issuers: HashSet<String>,
    pub(super) revoked_serials: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct ReloadableTrustStore {
    state: Arc<RwLock<TrustState>>,
}

impl ReloadableTrustStore {
    pub fn new(trusted_issuers: impl IntoIterator<Item = String>) -> Self {
        Self {
            state: Arc::new(RwLock::new(TrustState {
                generation: 1,
                trusted_issuers: trusted_issuers.into_iter().collect(),
                revoked_serials: HashSet::new(),
            })),
        }
    }

    pub fn reload(
        &self,
        trusted_issuers: impl IntoIterator<Item = String>,
        revoked_serials: impl IntoIterator<Item = String>,
    ) -> Result<u64, TlsError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| TlsError::TrustStateUnavailable)?;
        state.generation = state.generation.saturating_add(1);
        state.trusted_issuers = trusted_issuers.into_iter().collect();
        state.revoked_serials = revoked_serials.into_iter().collect();
        Ok(state.generation)
    }

    pub fn verify_now(&self, handshake: &TlsHandshake) -> Result<VerifiedPeer, TlsError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.verify(handshake, now)
    }
}

impl ClientCertificateVerifier for ReloadableTrustStore {
    fn verify(&self, handshake: &TlsHandshake, now_unix: u64) -> Result<VerifiedPeer, TlsError> {
        if handshake.version != TlsVersion::V1_3 {
            return Err(TlsError::VersionRequired);
        }
        if handshake.alpn != REQUIRED_ALPN {
            return Err(TlsError::ProtocolRequired);
        }
        let certificate = handshake
            .peer_chain
            .first()
            .ok_or(TlsError::ClientCertificateRequired)?;
        let state = self
            .state
            .read()
            .map_err(|_| TlsError::TrustStateUnavailable)?;
        if !state
            .trusted_issuers
            .contains(&certificate.issuer_fingerprint)
        {
            return Err(TlsError::UntrustedIssuer);
        }
        if state.revoked_serials.contains(&certificate.serial) {
            return Err(TlsError::Revoked);
        }
        if now_unix < certificate.not_before_unix {
            return Err(TlsError::NotYetValid);
        }
        if now_unix >= certificate.not_after_unix {
            return Err(TlsError::Expired);
        }
        if certificate.dns_sans.is_empty() && certificate.uri_sans.is_empty() {
            return Err(TlsError::MissingIdentitySan);
        }
        Ok(VerifiedPeer {
            certificate: certificate.clone(),
            trust_generation: state.generation,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MtlsConfig {
    pub certificate_chain: PathBuf,
    pub private_key: PathBuf,
    pub client_ca_bundle: PathBuf,
    pub revoked_serials: HashSet<String>,
}

#[derive(Clone)]
pub struct ReloadableTlsAcceptor {
    state: Arc<RwLock<ConcreteTrustState>>,
}

pub(super) struct ConcreteTrustState {
    pub(super) generation: u64,
    pub(super) config: Arc<ServerConfig>,
    pub(super) revoked_serials: HashSet<String>,
}

impl ReloadableTlsAcceptor {
    pub fn new(config: MtlsConfig) -> Result<Self, String> {
        let state = load_server_config(&config, 1)?;
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
        })
    }

    pub fn reload(&self, config: MtlsConfig) -> Result<u64, String> {
        let generation = self
            .state
            .read()
            .map_err(|_| "TLS trust state lock poisoned")?
            .generation
            .saturating_add(1);
        let replacement = load_server_config(&config, generation)?;
        *self
            .state
            .write()
            .map_err(|_| "TLS trust state lock poisoned")? = replacement;
        Ok(generation)
    }

    pub async fn accept(
        &self,
        stream: TcpStream,
    ) -> Result<(TlsStream<TcpStream>, VerifiedPeer), String> {
        let (config, revoked, generation) = {
            let state = self
                .state
                .read()
                .map_err(|_| "TLS trust state lock poisoned")?;
            (
                state.config.clone(),
                state.revoked_serials.clone(),
                state.generation,
            )
        };
        let stream = TlsAcceptor::from(config)
            .accept(stream)
            .await
            .map_err(|error| format!("TLS handshake rejected: {error}"))?;
        let (_, connection) = stream.get_ref();
        if connection.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
            return Err("TLS 1.3 is required".into());
        }
        if connection.alpn_protocol() != Some(REQUIRED_ALPN.as_bytes()) {
            return Err("required ALPN was not negotiated".into());
        }
        let leaf = connection
            .peer_certificates()
            .and_then(|chain| chain.first())
            .ok_or_else(|| "client certificate is required".to_string())?;
        let certificate = peer_from_der(leaf)?;
        if revoked.contains(&certificate.serial) {
            return Err("client certificate is revoked".into());
        }
        if certificate.dns_sans.is_empty() && certificate.uri_sans.is_empty() {
            return Err("client certificate has no identity SAN".into());
        }
        Ok((
            stream,
            VerifiedPeer {
                certificate,
                trust_generation: generation,
            },
        ))
    }
}
