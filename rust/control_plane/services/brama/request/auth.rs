//! Proving to the gateway which agent is calling, without putting the shared
//! secret on the wire.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use std::collections::BTreeMap;
use std::env::VarError;

use crate::agent::credential::SECRET;

/// The variable naming the agent a signed request speaks for.
const AGENT_ID: &str = "WISENT_APP_AGENT_ID";

/// The agent id and shared secret requests are signed with; none when no secret
/// is configured, so the caller is unsigned. The agent id is the configured one,
/// else the agent `/setup` declares. A variable that is set but not Unicode is
/// refused naming it.
pub(crate) fn caller_credentials() -> Result<Option<(String, String)>, String> {
    let secret = match std::env::var(SECRET) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) | Err(VarError::NotPresent) => return Ok(None),
        Err(VarError::NotUnicode(_)) => {
            return Err(format!("{SECRET} cannot be read: value is not Unicode"))
        }
    };
    let agent_id = match std::env::var(AGENT_ID) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(VarError::NotPresent) => crate::slash::setup::DEFAULT_AGENT_ID.to_owned(),
        Err(VarError::NotUnicode(_)) => {
            return Err(format!("{AGENT_ID} cannot be read: value is not Unicode"))
        }
    };
    Ok(Some((agent_id, secret)))
}

/// Signs a request to Brama for the configured agent. An unsigned caller adds no
/// headers; a body that is not UTF-8 or a signature that cannot be made is refused,
/// because sending the request unsigned would be answered as another caller.
pub(crate) fn insert_caller_auth_headers(
    headers: &mut BTreeMap<String, String>,
    body: &[u8],
) -> Result<(), String> {
    let Some((agent_id, secret)) = caller_credentials()? else {
        return Ok(());
    };
    let body = std::str::from_utf8(body)
        .map_err(|error| format!("the request body Brama must sign is not UTF-8: {error}"))?;
    let (timestamp, body_hash, signature) =
        crate::model_router::hmac_headers(body, &agent_id, &secret)
            .map_err(|error| format!("the request to Brama cannot be signed: {error}"))?;
    headers.insert("x-agent-id".into(), agent_id);
    headers.insert("x-agent-timestamp".into(), timestamp);
    headers.insert("x-agent-body-sha256".into(), body_hash);
    headers.insert("x-agent-signature".into(), signature);
    Ok(())
}
