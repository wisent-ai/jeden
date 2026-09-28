//! The first frame of a connection that presented no client certificate:
//! `identity/authenticate` with the Wisent access token the person signed in
//! with and the organization they act for. Nothing else is served before it.

use super::*;
use crate::rpc::PROTOCOL_VERSION;

const AUTHENTICATE_METHOD: &str = "identity/authenticate";

impl<B: SessionBackend> HeadlessDaemon<B> {
    /// Answer the first frame of a certificate-less connection. `Ok` carries
    /// the authenticated connection and the success response; `Err` is the
    /// refusal to send before closing.
    pub(super) async fn authenticate_member(
        &self,
        frame: &[u8],
        trust_generation: u64,
    ) -> Result<(AuthenticatedConnection, Value), Value> {
        let request: RequestEnvelopeV1 = serde_json::from_slice(frame).map_err(|error| {
            refusal(Value::Null, "malformed_json", error.to_string(), false)
        })?;
        let id = Value::String(request.id.clone());
        if request.meta.protocol_version != PROTOCOL_VERSION {
            return Err(refusal(
                id,
                "unsupported_protocol",
                format!("protocolVersion must be {PROTOCOL_VERSION}"),
                false,
            ));
        }
        if request.method != AUTHENTICATE_METHOD {
            return Err(refusal(
                id,
                "unauthenticated",
                format!(
                    "no client certificate was presented; the first request must be {AUTHENTICATE_METHOD}"
                ),
                false,
            ));
        }
        let Some(authority) = self.identity.as_ref() else {
            return Err(refusal(
                id,
                "unauthenticated",
                "this daemon admits client certificates only; its identity map names no Wisent organization".into(),
                false,
            ));
        };
        let field = |name: &str| {
            request
                .params
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let (token, organization) = (field("accessToken"), field("organizationId"));
        let member = authority
            .authorize(&token, &organization)
            .await
            .map_err(|refused: IdentityRefusal| {
                refusal(
                    id.clone(),
                    refused.code(),
                    refused.message(),
                    refused.retryable(),
                )
            })?;
        let identity = self.directory.resolve_member(&member).map_err(|error| match error {
            TenantError::IdentityNotMapped => refusal(
                id.clone(),
                "access_denied",
                format!(
                    "organization {} is not in this daemon's identity map",
                    member.organization_id
                ),
                false,
            ),
            other => refusal(id.clone(), "internal", format!("{other:?}"), true),
        })?;
        let response = json!({
            "id": id,
            "result": {
                "principal": identity.principal.as_str(),
                "tenant": identity.tenant.as_str(),
                "organizationId": member.organization_id.to_string(),
                "role": member.role,
            }
        });
        Ok((
            AuthenticatedConnection {
                identity,
                trust_generation,
            },
            response,
        ))
    }
}

fn refusal(id: Value, code: &str, message: String, retryable: bool) -> Value {
    wire_error(
        id,
        ErrorV1 {
            code: code.into(),
            message,
            retryable,
            details: json!({}),
        },
    )
}
