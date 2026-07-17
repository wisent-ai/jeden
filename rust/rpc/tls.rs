use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;

use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
    generation: u64,
    trusted_issuers: HashSet<String>,
    revoked_serials: HashSet<String>,
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

struct ConcreteTrustState {
    generation: u64,
    config: Arc<ServerConfig>,
    revoked_serials: HashSet<String>,
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

fn load_server_config(source: &MtlsConfig, generation: u64) -> Result<ConcreteTrustState, String> {
    let certificates = load_certificates(&source.certificate_chain)?;
    let key = load_private_key(&source.private_key)?;
    let mut roots = RootCertStore::empty();
    for certificate in load_certificates(&source.client_ca_bundle)? {
        roots
            .add(certificate)
            .map_err(|error| format!("invalid client CA certificate: {error}"))?;
    }
    if roots.is_empty() {
        return Err("client CA bundle is empty".into());
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|error| format!("invalid client verifier: {error}"))?;
    let mut config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates, key)
        .map_err(|error| format!("invalid server identity: {error}"))?;
    config.alpn_protocols = vec![REQUIRED_ALPN.as_bytes().to_vec()];
    Ok(ConcreteTrustState {
        generation,
        config: Arc::new(config),
        revoked_serials: source.revoked_serials.clone(),
    })
}

fn load_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let certificates = rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    if certificates.is_empty() {
        Err(format!("certificate file is empty: {}", path.display()))
    } else {
        Ok(certificates)
    }
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    rustls_pemfile::private_key(&mut BufReader::new(file))
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?
        .ok_or_else(|| format!("private key file is empty: {}", path.display()))
}

fn peer_from_der(der: &CertificateDer<'_>) -> Result<PeerCertificate, String> {
    let (_, certificate) = parse_x509_certificate(der.as_ref())
        .map_err(|error| format!("invalid client certificate: {error}"))?;
    let mut dns_sans = Vec::new();
    let mut uri_sans = Vec::new();
    if let Ok(Some(extension)) = certificate.subject_alternative_name() {
        for name in &extension.value.general_names {
            match name {
                GeneralName::DNSName(value) => dns_sans.push((*value).to_owned()),
                GeneralName::URI(value) => uri_sans.push((*value).to_owned()),
                _ => {}
            }
        }
    }
    Ok(PeerCertificate {
        serial: certificate.raw_serial_as_string(),
        issuer_fingerprint: String::new(),
        dns_sans,
        uri_sans,
        not_before_unix: certificate.validity().not_before.timestamp().max(0) as u64,
        not_after_unix: certificate.validity().not_after.timestamp().max(0) as u64,
    })
}
