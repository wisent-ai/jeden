//! Deciding what a token is allowed to do before the relay acts on a
//! request.
//!
//! Split out of `collab/relay.rs`, which had grown past the module line cap.

use super::RelayStore;
use reqwest::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};

fn answered(status: StatusCode, body: serde_json::Value) -> (u16, String) {
    (status.as_u16(), body.to_string())
}

fn refused(status: StatusCode, error: impl std::fmt::Display) -> (u16, String) {
    answered(status, json!({"ok": false, "error": error.to_string()}))
}

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
            Ok(health) => answered(StatusCode::OK, health),
            Err(error) => refused(StatusCode::INTERNAL_SERVER_ERROR, error),
        };
    }
    let target = match path.strip_prefix("/room/") {
        Some(room) if !room.is_empty() => room,
        _ => return refused(StatusCode::NOT_FOUND, format!("{path} names no room")),
    };
    if method == "PUT" {
        if let Some(room) = target.strip_suffix("/token") {
            return match token {
                Some(old) => match store.rotate_token(room, old, body.trim()) {
                    Ok(true) => answered(StatusCode::OK, json!({"ok": true})),
                    Ok(false) => refused(StatusCode::FORBIDDEN, "unauthorized"),
                    Err(error) => refused(StatusCode::BAD_REQUEST, error),
                },
                None => refused(StatusCode::FORBIDDEN, "write token required"),
            };
        }
    }
    match method {
        "POST" => {
            // The body's size was held to the relay's limit before it was read.
            let blob = body.trim();
            if blob.is_empty() {
                return refused(StatusCode::BAD_REQUEST, "empty body");
            }
            match store.post_authorized(target, blob.to_string(), token) {
                Ok(seq) => answered(StatusCode::OK, json!({"ok": true, "seq": seq})),
                Err(error) => refused(StatusCode::FORBIDDEN, error),
            }
        }
        "GET" => match parse_since(query) {
            Err(error) => refused(StatusCode::BAD_REQUEST, error),
            Ok(since) => match store.get(target, since) {
                Ok((events, next)) => {
                    answered(StatusCode::OK, json!({"ok": true, "events": events, "next": next}))
                }
                Err(error) => refused(StatusCode::INTERNAL_SERVER_ERROR, error),
            },
        },
        _ => refused(StatusCode::METHOD_NOT_ALLOWED, format!("{method} is not a relay method")),
    }
}

/// The `since=` cursor of a read: none when the query states none, so the
/// read starts at the room's first event; refused when it is not a number.
pub(super) fn parse_since(query: &str) -> Result<Option<usize>, String> {
    match query.split('&').find_map(|pair| pair.strip_prefix("since=")) {
        None => Ok(None),
        Some(since) => since
            .parse()
            .map(Some)
            .map_err(|error| format!("since={since} is not an event sequence: {error}")),
    }
}

/// The role a write token is bound to: the declared role its prefix names.
pub(crate) fn token_role(token: &str) -> Option<crate::collab::CollabRole> {
    let (role, _) = token.split_once('.')?;
    crate::collab::CollabRole::named(role)
}
pub(super) fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
/// Milliseconds since the Unix epoch for room and frame rows, or why the
/// clock has none to give.
pub(super) fn now_ms() -> Result<i64, String> {
    crate::clock::unix_millis_i64().map_err(|error| error.to_string())
}
