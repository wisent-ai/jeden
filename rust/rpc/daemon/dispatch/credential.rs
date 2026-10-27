//! Host-wide model credentials require an explicit operator-nominated user.
//! A tenant/workspace grant alone is never sufficient.
use super::{protocol_error, AuthenticatedConnection, ErrorV1, Value};
use std::sync::OnceLock;

#[allow(clippy::result_large_err)]
pub(super) fn request(
    connection: &AuthenticatedConnection,
    method: &str,
    params: &Value,
) -> Result<Value, ErrorV1> {
    static MANAGER: OnceLock<Result<Option<String>, String>> = OnceLock::new();
    let configured = MANAGER.get_or_init(|| {
        let Some(raw) = std::env::var_os("JEDEN_CREDENTIAL_MANAGER_USER_ID") else {
            return Ok(None);
        };
        let text = raw
            .into_string()
            .map_err(|raw| format!("JEDEN_CREDENTIAL_MANAGER_USER_ID {raw:?} is not Unicode"))?;
        let user = uuid::Uuid::parse_str(text.trim())
            .map_err(|error| format!("JEDEN_CREDENTIAL_MANAGER_USER_ID is not a UUID: {error}"))?;
        Ok(Some(format!("wisent-user:{user}")))
    });
    match configured {
        Err(cause) => return Err(protocol_error("credential_configuration", cause)),
        Ok(None) => return Err(protocol_error(
            "access_denied",
            "host credential management is disabled; the host operator must set JEDEN_CREDENTIAL_MANAGER_USER_ID",
        )),
        Ok(Some(manager)) if manager.as_str() == connection.identity.principal.as_str() => {}
        Ok(Some(_)) => return Err(protocol_error(
            "access_denied",
            "this identity is not the host's nominated credential manager",
        )),
    }
    crate::rpc::server::credential_request(method, params)
        .map_err(|(code, cause)| protocol_error(code, &cause))
}
