//! TLS on the session port: who is allowed to connect, and the trust state
//! that can be reloaded without dropping the listener. A peer proves itself
//! with a client certificate, or, where the identity map names Wisent
//! organizations, with a Wisent identity in its first frame.

use rustls::ServerConfig;
use std::collections::HashSet;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
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

/// What the TLS handshake proved about the peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsPeer {
    /// A client certificate the trust store verified.
    Certificate(VerifiedPeer),
    /// No certificate was presented; the connection must authenticate with a
    /// Wisent identity before any other request is served.
    Anonymous { trust_generation: u64 },
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
    /// The host clock gave no time to check the certificate's validity
    /// against; read as zero, every certificate read as not yet valid.
    ClockUnreadable(String),
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
                generation: NonZeroU64::MIN.get(),
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
        // The new sets are built before the lock is taken and stored together,
        // so no holder can panic between them: a poisoned lock still holds one
        // whole trust state, and is taken back rather than refusing every
        // later handshake.
        let trusted_issuers = trusted_issuers.into_iter().collect();
        let revoked_serials = revoked_serials.into_iter().collect();
        let mut state = crate::panicked::written(&self.state);
        state.generation = state.generation.saturating_add(NonZeroU64::MIN.get());
        state.trusted_issuers = trusted_issuers;
        state.revoked_serials = revoked_serials;
        Ok(state.generation)
    }

    pub fn verify_now(&self, handshake: &TlsHandshake) -> Result<VerifiedPeer, TlsError> {
        let now = crate::clock::unix_seconds()
            .map_err(|error| TlsError::ClockUnreadable(error.to_string()))?;
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
        let state = crate::panicked::read_held(&self.state);
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
    /// Finish handshakes that present no client certificate, for peers that
    /// authenticate with a Wisent identity instead.
    pub admit_without_certificate: bool,
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
        let state = load_server_config(&config, NonZeroU64::MIN.get())?;
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
        })
    }

    pub fn reload(&self, config: MtlsConfig) -> Result<u64, String> {
        // Built outside the lock and replaced whole, so a poisoned lock still
        // holds one configuration; taken back, the daemon keeps accepting.
        let generation = crate::panicked::read_held(&self.state)
            .generation
            .saturating_add(NonZeroU64::MIN.get());
        let replacement = load_server_config(&config, generation)?;
        *crate::panicked::written(&self.state) = replacement;
        Ok(generation)
    }

    pub async fn accept(
        &self,
        stream: TcpStream,
    ) -> Result<(TlsStream<TcpStream>, TlsPeer), String> {
        let (config, revoked, generation) = {
            let state = crate::panicked::read_held(&self.state);
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
        let Some(leaf) = connection
            .peer_certificates()
            .and_then(|chain| chain.first())
        else {
            return Ok((
                stream,
                TlsPeer::Anonymous {
                    trust_generation: generation,
                },
            ));
        };
        let certificate = peer_from_der(leaf)?;
        if revoked.contains(&certificate.serial) {
            return Err("client certificate is revoked".into());
        }
        if certificate.dns_sans.is_empty() && certificate.uri_sans.is_empty() {
            return Err("client certificate has no identity SAN".into());
        }
        Ok((
            stream,
            TlsPeer::Certificate(VerifiedPeer {
                certificate,
                trust_generation: generation,
            }),
        ))
    }
}
