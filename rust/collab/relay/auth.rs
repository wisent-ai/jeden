//! Deciding what a token is allowed to do before the relay acts on a
//! request.
//!
//! Split out of `collab/relay.rs`, which had grown past the module line cap.

use super::RelayStore;
use crate::collab::MAX_BLOB_BYTES;
use serde_json::json;
use sha2::{Digest, Sha256};

pub(super) fn relay_response_authorized(
    store: &RelayStore,
    method: &str,
    path: &str,
    query: &str,
    body: &str,
    token: Option<&str>,
) -> (u16, String) {
    if method == "GET" && path == "/health" {
        return match store.health() {
            Ok(v) => (200, v.to_string()),
            Err(e) => (500, json!({"ok":false,"error":e}).to_string()),
        };
    }
    let target = match path.strip_prefix("/room/") {
        Some(r) if !r.is_empty() => r,
        _ => return (404, json!({"ok":false,"error":"not found"}).to_string()),
    };
    if method == "PUT" {
        if let Some(room) = target.strip_suffix("/token") {
            return match token {
                Some(old) => match store.rotate_token(room, old, body.trim()) {
                    Ok(true) => (200, json!({"ok":true}).to_string()),
                    Ok(false) => (403, json!({"ok":false,"error":"unauthorized"}).to_string()),
                    Err(e) => (400, json!({"ok":false,"error":e}).to_string()),
                },
                None => (
                    403,
                    json!({"ok":false,"error":"write token required"}).to_string(),
                ),
            };
        }
    }
    match method {
        "POST" => {
            let blob = body.trim();
            if blob.is_empty() {
                return (400, json!({"ok":false,"error":"empty body"}).to_string());
            }
            if blob.len() > MAX_BLOB_BYTES {
                return (
                    413,
                    json!({"ok":false,"error":"payload too large"}).to_string(),
                );
            }
            match store.post_authorized(target, blob.to_string(), token) {
                Ok(Some(seq)) => (200, json!({"ok":true,"seq":seq}).to_string()),
                Ok(None) => (429, json!({"ok":false,"error":"room is full"}).to_string()),
                Err(e) => (403, json!({"ok":false,"error":e}).to_string()),
            }
        }
        "GET" => {
            let (events, next) = store.get(target, parse_since(query));
            (
                200,
                json!({"ok":true,"events":events,"next":next}).to_string(),
            )
        }
        _ => (
            405,
            json!({"ok":false,"error":"method not allowed"}).to_string(),
        ),
    }
}
pub(super) fn parse_since(query: &str) -> usize {
    query
        .split('&')
        .find_map(|p| p.strip_prefix("since="))
        .and_then(|v| v.parse().ok())
        .unwrap_or_default()
}

pub(crate) fn token_role(token: &str) -> Option<&str> {
    let (role, _) = token.split_once('.')?;
    matches!(role, "view" | "prompt" | "abort" | "full").then_some(role)
}
pub(super) fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
