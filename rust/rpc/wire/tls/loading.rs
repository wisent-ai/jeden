//! Reading certificates and keys off disk, and turning a peer certificate
//! into the few facts this product decides on.
//!
//! Split out of `rpc/wire/tls.rs`, which had grown past the module line cap.

use super::{ConcreteTrustState, MtlsConfig, PeerCertificate};
use crate::rpc::wire::tls::REQUIRED_ALPN;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;

/// rustls 0.23 refuses to guess a process-level provider when more than one is
/// linked, and both are here: `ring` and `aws-lc-rs` arrive through different
/// feature paths of the reqwest stack this binary already carries. The refusal is
/// a panic at the first TLS call, which is what `jeden headless` did on every
/// start. The listener therefore names its provider once, before any config
/// exists; an `Err` means another component installed one first, which is the
/// same outcome this wants.
static CRYPTO_PROVIDER: LazyLock<()> = LazyLock::new(|| {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
});

pub(super) fn load_server_config(
    source: &MtlsConfig,
    generation: u64,
) -> Result<ConcreteTrustState, String> {
    LazyLock::force(&CRYPTO_PROVIDER);
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

pub(super) fn peer_from_der(der: &CertificateDer<'_>) -> Result<PeerCertificate, String> {
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
