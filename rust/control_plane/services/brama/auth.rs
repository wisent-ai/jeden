//! Proving to the gateway which agent is calling, without putting the shared
//! secret on the wire.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use std::collections::BTreeMap;

pub(crate) fn caller_credentials() -> Option<(String, String)> {
    let secret = std::env::var("WISENT_APP_AGENT_AUTH_SECRET")
        .ok()
        .filter(|value| !value.is_empty())?;
    let agent_id = std::env::var("WISENT_APP_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "wisent-app".into());
    Some((agent_id, secret))
}

pub(crate) fn insert_caller_auth_headers(headers: &mut BTreeMap<String, String>, body: &[u8]) {
    let Some((agent_id, secret)) = caller_credentials() else {
        return;
    };
    let Ok(body) = std::str::from_utf8(body) else {
        return;
    };
    let Ok((timestamp, body_hash, signature)) =
        crate::model_router::hmac_headers(body, &agent_id, &secret)
    else {
        return;
    };
    headers.insert("x-agent-id".into(), agent_id);
    headers.insert("x-agent-timestamp".into(), timestamp);
    headers.insert("x-agent-body-sha256".into(), body_hash);
    headers.insert("x-agent-signature".into(), signature);
}
