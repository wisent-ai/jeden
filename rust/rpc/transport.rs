use super::tenant::{TenantDirectory, TenantError, TenantPrincipal};
use super::tls::{ClientCertificateVerifier, TlsError, TlsHandshake};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;

pub const PROTOCOL_VERSION: &str = "jeden.session.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMetaV1 {
    pub protocol_version: String,
    pub idempotency_key: String,
    pub deadline_unix_millis: Option<u64>,
    pub trace_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEnvelopeV1 {
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
    pub meta: RequestMetaV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorV1 {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Tls(TlsError),
    Tenant(TenantError),
    ProtocolVersion,
    InvalidReconnectToken,
    ExpiredReconnectToken,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedConnection {
    pub identity: TenantPrincipal,
    pub trust_generation: u64,
}

impl AuthenticatedConnection {
    pub fn accept(
        handshake: &TlsHandshake,
        now_unix: u64,
        verifier: &dyn ClientCertificateVerifier,
        directory: &TenantDirectory,
    ) -> Result<Self, TransportError> {
        let verified = verifier
            .verify(handshake, now_unix)
            .map_err(TransportError::Tls)?;
        let identity = directory
            .resolve(&verified)
            .map_err(TransportError::Tenant)?;
        Ok(Self {
            identity,
            trust_generation: verified.trust_generation,
        })
    }

    pub fn validate_request(&self, request: &RequestEnvelopeV1) -> Result<(), TransportError> {
        if request.meta.protocol_version != PROTOCOL_VERSION {
            Err(TransportError::ProtocolVersion)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReconnectClaims {
    version: u8,
    principal: String,
    tenant: String,
    session_id: String,
    expires_unix: u64,
}

#[derive(Clone)]
pub struct ReconnectTokens {
    key: Arc<[u8]>,
}

impl ReconnectTokens {
    pub fn new(key: impl Into<Vec<u8>>) -> Result<Self, TransportError> {
        let key = key.into();
        if key.len() < 32 {
            return Err(TransportError::InvalidReconnectToken);
        }
        Ok(Self {
            key: Arc::from(key),
        })
    }

    pub fn issue(
        &self,
        connection: &AuthenticatedConnection,
        session_id: &str,
        expires_unix: u64,
    ) -> Result<String, TransportError> {
        if session_id.is_empty() {
            return Err(TransportError::InvalidReconnectToken);
        }
        let claims = ReconnectClaims {
            version: 1,
            principal: connection.identity.principal.as_str().to_owned(),
            tenant: connection.identity.tenant.as_str().to_owned(),
            session_id: session_id.to_owned(),
            expires_unix,
        };
        let payload =
            serde_json::to_vec(&claims).map_err(|_| TransportError::InvalidReconnectToken)?;
        let signature = self.sign(&payload)?;
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    pub fn verify(
        &self,
        connection: &AuthenticatedConnection,
        token: &str,
        now_unix: u64,
    ) -> Result<String, TransportError> {
        let (payload, signature) = token
            .split_once('.')
            .ok_or(TransportError::InvalidReconnectToken)?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| TransportError::InvalidReconnectToken)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| TransportError::InvalidReconnectToken)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
            .map_err(|_| TransportError::InvalidReconnectToken)?;
        mac.update(&payload);
        mac.verify_slice(&signature)
            .map_err(|_| TransportError::InvalidReconnectToken)?;
        let claims: ReconnectClaims =
            serde_json::from_slice(&payload).map_err(|_| TransportError::InvalidReconnectToken)?;
        if claims.version != 1
            || claims.principal != connection.identity.principal.as_str()
            || claims.tenant != connection.identity.tenant.as_str()
        {
            return Err(TransportError::InvalidReconnectToken);
        }
        if now_unix >= claims.expires_unix {
            return Err(TransportError::ExpiredReconnectToken);
        }
        Ok(claims.session_id)
    }

    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, TransportError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
            .map_err(|_| TransportError::InvalidReconnectToken)?;
        mac.update(payload);
        Ok(mac.finalize().into_bytes().to_vec())
    }
}
