//! Serving the relay over plain sockets, reading each request itself.
//!
//! Split out of `collab/relay.rs`, which had grown past the module line cap.

use super::super::MAX_BLOB_BYTES;
use super::{relay_response_authorized, RelayStore};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use crate::collab::relay::auth::token_role;
use serde_json::json;

pub fn serve(addr: &str) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    let store = std::sync::Arc::new(RelayStore::new());
    println!(
        "jeden collab-relay listening on http://{bound} (durable {})",
        store.path().display()
    );
    for stream in listener.incoming().flatten() {
        let store = store.clone();
        std::thread::spawn(move || {
            let _ = handle_conn(stream, &store);
        });
    }
    Ok(())
}
pub(super) fn handle_conn(mut stream: TcpStream, store: &RelayStore) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            break buf.len();
        }
    };
    let header = String::from_utf8_lossy(&buf[..header_end.min(buf.len())]).to_string();
    let (method, path, query) = parse_request_line(&header);
    let length = parse_content_length(&header);
    if length > MAX_BLOB_BYTES {
        write_response(
            &mut stream,
            413,
            &json!({"ok":false,"error":"payload too large"}).to_string(),
        )?;
        return Ok(());
    }
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n])
    }
    let body = String::from_utf8_lossy(&buf[header_end.min(buf.len())..]).to_string();
    let token = header_value(&header, "x-jeden-write-token");
    let requested_role = header_value(&header, "x-jeden-role");
    let (status, response) = match (token.as_deref(), requested_role.as_deref()) {
        (Some(token), Some(role)) if token_role(token) != Some(role) => (
            403,
            json!({"ok":false,"error":"write token is not valid for requested role"}).to_string(),
        ),
        _ => relay_response_authorized(store, &method, &path, &query, &body, token.as_deref()),
    };
    write_response(&mut stream, status, &response)
}
pub(super) fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let response=format!("HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\naccess-control-allow-origin: *\r\naccess-control-allow-headers: content-type,x-jeden-write-token,x-jeden-role\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",body.len(),body);
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
pub(super) fn find_subsequence(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}
pub(super) fn parse_request_line(header: &str) -> (String, String, String) {
    let mut p = header.lines().next().unwrap_or("").split_whitespace();
    let method = p.next().unwrap_or("").to_string();
    let target = p.next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (method, path.to_string(), query.to_string())
}
pub(super) fn parse_content_length(header: &str) -> usize {
    header_value(header, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or_default()
}

fn header_value(header: &str, name: &str) -> Option<String> {
    header.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}
