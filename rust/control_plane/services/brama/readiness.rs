//! Brama readiness checks provider credentials, not only process health.

use super::super::transport::{SecretRef, TransportRequest};
use super::{BramaClient, BramaError, BramaReadiness};
use serde_json::Value;
use std::collections::BTreeMap;

impl BramaClient {
    /// `/readyz` redeems one credential per configured provider; `/health` and
    /// `/v1/models` alone do not establish that a subscription can answer.
    pub fn readiness(&self) -> Result<BramaReadiness, BramaError> {
        let endpoint = self.key()?;
        let mut headers = BTreeMap::new();
        if let Some(token) = SecretRef::resolve_optional(self.authorization.as_ref())
            .map_err(BramaError::Configuration)?
        {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        let response = self
            .transport
            .execute(TransportRequest {
                method: reqwest::Method::GET,
                url: format!("{endpoint}/readyz"),
                headers,
                body: None,
            })
            .map_err(BramaError::Transport)?;
        let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
            BramaError::InvalidResponse(format!("{endpoint}/readyz is not JSON: {error}"))
        })?;
        let field = |name: &str| {
            value.get(name).ok_or_else(|| {
                BramaError::InvalidResponse(format!("{endpoint}/readyz lacks {name}"))
            })
        };
        let flag = |name: &str| -> Result<bool, BramaError> {
            field(name)?.as_bool().ok_or_else(|| {
                BramaError::InvalidResponse(format!("{endpoint}/readyz {name} is not a boolean"))
            })
        };
        let reason = field("reason")?.as_str().ok_or_else(|| {
            BramaError::InvalidResponse(format!("{endpoint}/readyz reason is not a string"))
        })?;
        let providers = field("providers")?.as_array().ok_or_else(|| {
            BramaError::InvalidResponse(format!("{endpoint}/readyz providers is not an array"))
        })?;
        let mut providers_without_credential = Vec::new();
        for (index, provider) in providers.iter().enumerate() {
            let name = provider
                .get("provider")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BramaError::InvalidResponse(format!(
                        "{endpoint}/readyz providers[{index}].provider is not a string"
                    ))
                })?;
            let credential = provider
                .get("credential")
                .and_then(Value::as_bool)
                .ok_or_else(|| {
                    BramaError::InvalidResponse(format!(
                        "{endpoint}/readyz providers[{index}].credential is not a boolean"
                    ))
                })?;
            if !credential {
                providers_without_credential.push(name.to_owned());
            }
        }
        Ok(BramaReadiness {
            status: response.status,
            ready: flag("ready")?,
            degraded: flag("degraded")?,
            operator_action_required: flag("operator_action_required")?,
            reason: reason.to_owned(),
            providers_without_credential,
        })
    }
}
