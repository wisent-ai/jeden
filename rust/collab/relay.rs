use parking_lot::Mutex;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use super::{MAX_BLOB_BYTES, MAX_ROOM_EVENTS};

/// In-memory per-room append-only log of opaque blobs.
#[derive(Default)]
pub struct RelayStore {
    rooms: Mutex<BTreeMap<String, Vec<String>>>,
}

impl RelayStore {
    pub fn new() -> Self {
        Self { rooms: Mutex::new(BTreeMap::new()) }
    }

    /// Append a blob to a room; returns the new sequence length, or `None` if
    /// the room is already at `MAX_ROOM_EVENTS` (backpressure).
    pub fn post(&self, room: &str, blob: String) -> Option<usize> {
        let mut rooms = self.rooms.lock();
        let log = rooms.entry(room.to_string()).or_default();
        if log.len() >= MAX_ROOM_EVENTS {
            return None;
        }
        log.push(blob);
        Some(log.len())
    }

    /// Blobs at index >= `since`, plus the next cursor (total length).
    pub fn get(&self, room: &str, since: usize) -> (Vec<String>, usize) {
        let rooms = self.rooms.lock();
        match rooms.get(room) {
            Some(log) => {
                let slice = log.iter().skip(since).cloned().collect();
                (slice, log.len())
            }
            None => (Vec::new(), 0),
        }
    }
}

/// Compute the JSON response `(status, body)` for one relay request. Pure given
/// the store; the socket loop is a thin adapter over this.
/// Routes: `GET /health`; `POST /room/<id>` (body = blob) -> `{seq}`;
/// `GET /room/<id>?since=<n>` -> `{events, next}`.
pub fn relay_response(store: &RelayStore, method: &str, path: &str, query: &str, body: &str) -> (u16, String) {
    if method == "GET" && path == "/health" {
        return (200, json!({ "ok": true, "service": "jeden-collab-relay" }).to_string());
    }
    let room = match path.strip_prefix("/room/") {
        Some(r) if !r.is_empty() => r,
        _ => return (404, json!({ "ok": false, "error": "not found" }).to_string()),
    };
    match method {
        "POST" => {
            let blob = body.trim();
            if blob.is_empty() {
                return (400, json!({ "ok": false, "error": "empty body" }).to_string());
            }
            if blob.len() > MAX_BLOB_BYTES {
                return (413, json!({ "ok": false, "error": "payload too large" }).to_string());
            }
            match store.post(room, blob.to_string()) {
                Some(seq) => (200, json!({ "ok": true, "seq": seq }).to_string()),
                None => (413, json!({ "ok": false, "error": "room is full" }).to_string()),
            }
        }
        "GET" => {
            let since = parse_since(query);
            let (events, next) = store.get(room, since);
            (200, json!({ "ok": true, "events": events, "next": next }).to_string())
        }
        _ => (405, json!({ "ok": false, "error": "method not allowed" }).to_string()),
    }
}

/// Extract `since` from a `since=<n>` query string; defaults to 0.
fn parse_since(query: &str) -> usize {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("since="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Run the relay server, blocking forever. `addr` is e.g. `127.0.0.1:8877`.
pub fn serve(addr: &str) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    println!("jeden collab-relay listening on http://{bound}");
    let store = Arc::new(RelayStore::new());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let store = store.clone();
                std::thread::spawn(move || {
                    let _ = handle_conn(stream, &store);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

/// Read one HTTP/1.1 request from `stream`, dispatch it, and write the response.
fn handle_conn(mut stream: TcpStream, store: &RelayStore) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read until end of headers.
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 8 * 1024 * 1024 {
            break buf.len();
        }
    };
    let header_text = String::from_utf8_lossy(&buf[..header_end.min(buf.len())]).to_string();
    let (method, path, query) = parse_request_line(&header_text);
    let content_length = parse_content_length(&header_text);
    // Reject oversized bodies before buffering them.
    if content_length > MAX_BLOB_BYTES {
        let resp_body = json!({ "ok": false, "error": "payload too large" }).to_string();
        let response = format!(
            "HTTP/1.1 413 Payload Too Large\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            resp_body.len(),
            resp_body
        );
        stream.write_all(response.as_bytes())?;
        return stream.flush();
    }
    // Read the remaining body bytes.
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end.min(buf.len())..]).to_string();
    let (status, resp_body) = relay_response(store, &method, &path, &query, &body);
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        resp_body.len(),
        resp_body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse `METHOD /path?query HTTP/1.1` into `(method, path, query)`.
pub(super) fn parse_request_line(header_text: &str) -> (String, String, String) {
    let line = header_text.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    (method, path, query)
}

pub(super) fn parse_content_length(header_text: &str) -> usize {
    header_text
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}
