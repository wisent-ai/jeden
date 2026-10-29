//! Serving the relay over HTTP. `tiny_http` reads each request; this module
//! bounds its body, checks the requested role and answers what the relay
//! decided.
//!
//! Split out of `collab/relay.rs`, which had grown past the module line cap.

use super::{relay_response_authorized, RelayStore};
use crate::collab::relay::auth::token_role;
use reqwest::StatusCode;
use serde_json::json;
use std::io::Read;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tiny_http::{Header, Request, Response, Server};

/// The variable that states how many bytes one relay request body may hold.
const MAX_BLOB_BYTES_VARIABLE: &str = "JEDEN_RELAY_MAX_BLOB_BYTES";

/// The largest request body the relay accepts, as its operator states it:
/// refused by name when unset or not a positive whole number.
fn stated_max_blob_bytes() -> Result<usize, String> {
    let stated = std::env::var(MAX_BLOB_BYTES_VARIABLE).map_err(|error| {
        format!("{MAX_BLOB_BYTES_VARIABLE} must state how many bytes one relay request may carry ({error})")
    })?;
    stated
        .trim()
        .parse::<NonZeroUsize>()
        .map(NonZeroUsize::get)
        .map_err(|error| format!("{MAX_BLOB_BYTES_VARIABLE} {stated:?} is not a positive whole number of bytes: {error}"))
}

pub fn serve(addr: &str) -> Result<(), String> {
    let max_blob_bytes = stated_max_blob_bytes()?;
    let server = Server::http(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = server.server_addr();
    let store = Arc::new(RelayStore::new());
    println!(
        "jeden relay listening on http://{bound} (durable {}, at most {max_blob_bytes} bytes a request)",
        store.location()
    );
    for request in server.incoming_requests() {
        let store = store.clone();
        std::thread::spawn(move || {
            let peer = match request.remote_addr() {
                Some(peer) => peer.to_string(),
                None => "a peer with no network address".to_owned(),
            };
            if let Err(error) = handle(request, &store, max_blob_bytes) {
                eprintln!("jeden relay request from {peer} failed: {error}");
            }
        });
    }
    Ok(())
}

/// The text of the request header `name`, when the request carries it.
fn header_value(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str().to_owned())
}

fn handle(mut request: Request, store: &RelayStore, max_blob_bytes: usize) -> std::io::Result<()> {
    // The operator's stated limit bounds every body; one whose length is not
    // declared up front cannot be held to it.
    let length = match request.body_length() {
        Some(length) if length > max_blob_bytes => {
            return answer(request, StatusCode::PAYLOAD_TOO_LARGE, json!({"ok": false, "error": format!("the body of {length} bytes is larger than the relay's {max_blob_bytes}")}).to_string());
        }
        Some(length) => length,
        None => {
            return answer(
                request,
                StatusCode::LENGTH_REQUIRED,
                json!({"ok": false, "error": "the request declares no content-length"}).to_string(),
            );
        }
    };
    let mut body = String::with_capacity(length);
    request.as_reader().read_to_string(&mut body)?;
    let method = request.method().to_string();
    let (path, query) = match request.url().split_once('?') {
        Some((path, query)) => (path.to_owned(), query.to_owned()),
        None => (request.url().to_owned(), String::new()),
    };
    let token = header_value(&request, "x-jeden-write-token");
    let requested_role = header_value(&request, "x-jeden-role");
    let (status, response) = match (token.as_deref(), requested_role.as_deref()) {
        (Some(token), Some(role))
            if token_role(token).is_none()
                || token_role(token) != crate::collab::CollabRole::named(role) =>
        {
            (
                StatusCode::FORBIDDEN.as_u16(),
                json!({"ok":false,"error":"write token is not valid for requested role"})
                    .to_string(),
            )
        }
        _ => relay_response_authorized(store, &method, &path, &query, &body, token.as_deref()),
    };
    respond(request, status, response)
}

fn answer(request: Request, status: StatusCode, body: String) -> std::io::Result<()> {
    respond(request, status.as_u16(), body)
}

fn respond(request: Request, status: u16, body: String) -> std::io::Result<()> {
    let header = |field: &str, value: &str| {
        Header::from_bytes(field.as_bytes(), value.as_bytes()).map_err(|()| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("the relay response header {field}: {value} is not a valid HTTP header"),
            )
        })
    };
    request.respond(
        Response::from_string(body)
            .with_status_code(status)
            .with_header(header("content-type", "application/json")?)
            .with_header(header("access-control-allow-origin", "*")?)
            .with_header(header(
                "access-control-allow-headers",
                "content-type,x-jeden-write-token,x-jeden-role",
            )?),
    )
}
