//! Real-time collaboration relay: a content-agnostic HTTP message broker plus
//! end-to-end-encrypted client helpers. The relay stores only opaque base64
//! blobs per room and never sees plaintext or decryption keys (those live in
//! the client and travel in the `#key=` URL fragment, never over the wire).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use parking_lot::Mutex;
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Max size of a single relay blob (base64 E2EE payload). Rejects larger POSTs.
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;
/// Max buffered events per room before the relay applies backpressure.
pub const MAX_ROOM_EVENTS: usize = 10_000;

// ---------------------------------------------------------------------------
// E2EE blob helpers (pure, round-trippable)
// ---------------------------------------------------------------------------

/// Encrypt `plain` under `key`, returning `base64url(nonce(12) || ciphertext+tag)`.
/// A fresh random nonce is generated per call, so identical plaintext yields
/// distinct blobs.
pub fn encrypt_blob(key: &[u8; 32], plain: &[u8]) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|e| e.to_string())?;
    let mut framed = Vec::with_capacity(12 + ciphertext.len());
    framed.extend_from_slice(&nonce);
    framed.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(framed))
}

/// Reverse of [`encrypt_blob`]. Rejects a blob too short to contain a nonce.
pub fn decrypt_blob(key: &[u8; 32], blob: &str) -> Result<Vec<u8>, String> {
    let framed = URL_SAFE_NO_PAD
        .decode(blob.trim())
        .map_err(|e| e.to_string())?;
    if framed.len() < 12 + 16 {
        return Err("encrypted blob is too short".into());
    }
    let (nonce, ciphertext) = framed.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| "decryption failed (wrong key or corrupt blob)".to_string())
}

/// Encode a 32-byte key for a `#key=` URL fragment.
pub fn encode_key(key: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

/// Decode a `#key=` fragment back into a 32-byte key.
pub fn decode_key(text: &str) -> Result<[u8; 32], String> {
    let bytes = URL_SAFE_NO_PAD.decode(text.trim()).map_err(|e| e.to_string())?;
    if bytes.len() != 32 {
        return Err("relay key must be 32 bytes".into());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// A freshly generated random room id (hex) and 32-byte key.
pub fn new_room_and_key() -> (String, [u8; 32]) {
    let mut room = [0u8; 8];
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut room);
    rand::thread_rng().fill_bytes(&mut key);
    (hex::encode(room), key)
}

// ---------------------------------------------------------------------------
// Relay URL parsing (pure)
// ---------------------------------------------------------------------------

/// A parsed HTTP relay target: the server base (scheme+host, no trailing slash),
/// a room id, and an optional decryption key from the `#key=` fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayUrl {
    pub base: String,
    pub room: String,
    pub key: Option<[u8; 32]>,
}

/// Parse an HTTP relay URL of the form `http://host[:port][/room/<id>][#key=<k>]`.
/// A missing room yields an empty `room` (caller generates one for `start`).
pub fn parse_relay_url(text: &str) -> Result<RelayUrl, String> {
    let text = text.trim();
    if !(text.starts_with("http://") || text.starts_with("https://")) {
        return Err("relay URL must start with http:// or https://".into());
    }
    let (without_frag, key) = match text.split_once('#') {
        Some((head, frag)) => {
            let k = frag.strip_prefix("key=").ok_or("relay fragment must be #key=<k>")?;
            (head, Some(decode_key(k)?))
        }
        None => (text, None),
    };
    // Split scheme off, then the first path segment boundary.
    let scheme_end = without_frag.find("://").ok_or("malformed relay URL")? + 3;
    let (scheme, after) = without_frag.split_at(scheme_end);
    let (authority, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, ""),
    };
    if authority.is_empty() {
        return Err("relay URL is missing a host".into());
    }
    let base = format!("{}{}", scheme, authority.trim_end_matches('/'));
    let room = path
        .trim_matches('/')
        .strip_prefix("room/")
        .map(|r| r.trim_matches('/').to_string())
        .unwrap_or_default();
    Ok(RelayUrl { base, room, key })
}

// ---------------------------------------------------------------------------
// Relay store + request handling (pure over an in-memory store)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Relay server (blocking socket loop)
// ---------------------------------------------------------------------------

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
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
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
fn parse_request_line(header_text: &str) -> (String, String, String) {
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

fn parse_content_length(header_text: &str) -> usize {
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

// ---------------------------------------------------------------------------
// Relay client (blocking reqwest)
// ---------------------------------------------------------------------------

/// POST one opaque blob to `base/room/<room>`; returns the new sequence length.
pub fn relay_post(base: &str, room: &str, blob: &str) -> Result<usize, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/room/{}", base.trim_end_matches('/'), room);
    let response = client
        .post(&url)
        .header("content-type", "text/plain")
        .body(blob.to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("relay POST {}: {}", status.as_u16(), text));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    value
        .get("seq")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .ok_or_else(|| "relay POST returned no seq".to_string())
}

/// GET blobs from `base/room/<room>` at index >= `since`; returns `(blobs, next)`.
pub fn relay_get(base: &str, room: &str, since: usize) -> Result<(Vec<String>, usize), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/room/{}?since={}", base.trim_end_matches('/'), room, since);
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("relay GET {}: {}", status.as_u16(), text));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let next = value.get("next").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(0);
    Ok((events, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn encrypt_decrypt_round_trips() {
        let k = key();
        let blob = encrypt_blob(&k, b"hello collab").unwrap();
        assert_eq!(decrypt_blob(&k, &blob).unwrap(), b"hello collab");
    }

    #[test]
    fn encrypt_uses_fresh_nonce_per_call() {
        let k = key();
        let a = encrypt_blob(&k, b"same").unwrap();
        let b = encrypt_blob(&k, b"same").unwrap();
        assert_ne!(a, b, "distinct nonces must yield distinct blobs");
        assert_eq!(decrypt_blob(&k, &a).unwrap(), b"same");
        assert_eq!(decrypt_blob(&k, &b).unwrap(), b"same");
    }

    #[test]
    fn decrypt_rejects_wrong_key() {
        let blob = encrypt_blob(&key(), b"secret").unwrap();
        let wrong = [9u8; 32];
        assert!(decrypt_blob(&wrong, &blob).is_err());
    }

    #[test]
    fn store_post_get_and_since_cursor() {
        let store = RelayStore::new();
        assert_eq!(store.post("r", "a".into()), Some(1));
        assert_eq!(store.post("r", "b".into()), Some(2));
        let (events, next) = store.get("r", 0);
        assert_eq!(events, vec!["a", "b"]);
        assert_eq!(next, 2);
        let (tail, next2) = store.get("r", 1);
        assert_eq!(tail, vec!["b"]);
        assert_eq!(next2, 2);
        let (empty, _) = store.get("r", 2);
        assert!(empty.is_empty());
    }

    #[test]
    fn store_isolates_rooms() {
        let store = RelayStore::new();
        store.post("room-a", "x".into());
        let (events, _) = store.get("room-b", 0);
        assert!(events.is_empty());
    }

    #[test]
    fn relay_response_oversized_blob_rejected() {
        let store = RelayStore::new();
        let big = "x".repeat(MAX_BLOB_BYTES + 1);
        let (status, _) = relay_response(&store, "POST", "/room/abc", "", &big);
        assert_eq!(status, 413);
        let (events, _) = store.get("abc", 0);
        assert!(events.is_empty());
    }

    #[test]
    fn relay_response_post_then_get() {
        let store = RelayStore::new();
        let (status, body) = relay_response(&store, "POST", "/room/abc", "", "blob1");
        assert_eq!(status, 200);
        assert!(body.contains("\"seq\":1"));
        let (status, body) = relay_response(&store, "GET", "/room/abc", "since=0", "");
        assert_eq!(status, 200);
        assert!(body.contains("blob1"));
        assert!(body.contains("\"next\":1"));
    }

    #[test]
    fn relay_response_empty_post_rejected() {
        let store = RelayStore::new();
        let (status, _) = relay_response(&store, "POST", "/room/abc", "", "   ");
        assert_eq!(status, 400);
    }

    #[test]
    fn relay_response_unknown_route_404() {
        let store = RelayStore::new();
        let (status, _) = relay_response(&store, "GET", "/nope", "", "");
        assert_eq!(status, 404);
    }

    #[test]
    fn relay_response_health_ok() {
        let store = RelayStore::new();
        let (status, body) = relay_response(&store, "GET", "/health", "", "");
        assert_eq!(status, 200);
        assert!(body.contains("jeden-collab-relay"));
    }

    #[test]
    fn parse_relay_url_full() {
        let k = encode_key(&key());
        let url = format!("http://127.0.0.1:8877/room/deadbeef#key={}", k);
        let parsed = parse_relay_url(&url).unwrap();
        assert_eq!(parsed.base, "http://127.0.0.1:8877");
        assert_eq!(parsed.room, "deadbeef");
        assert_eq!(parsed.key, Some(key()));
    }

    #[test]
    fn parse_relay_url_base_only() {
        let parsed = parse_relay_url("http://host:9000").unwrap();
        assert_eq!(parsed.base, "http://host:9000");
        assert_eq!(parsed.room, "");
        assert_eq!(parsed.key, None);
    }

    #[test]
    fn parse_relay_url_rejects_non_http() {
        assert!(parse_relay_url("file:///tmp/x").is_err());
    }

    #[test]
    fn parse_request_line_splits_query() {
        let (m, p, q) = parse_request_line("GET /room/x?since=3 HTTP/1.1\r\nhost: y\r\n");
        assert_eq!(m, "GET");
        assert_eq!(p, "/room/x");
        assert_eq!(q, "since=3");
    }

    #[test]
    fn content_length_parsed_case_insensitively() {
        assert_eq!(parse_content_length("POST / HTTP/1.1\r\nContent-Length: 42\r\n"), 42);
        assert_eq!(parse_content_length("POST / HTTP/1.1\r\ncontent-length: 7\r\n"), 7);
        assert_eq!(parse_content_length("GET / HTTP/1.1\r\nhost: x\r\n"), 0);
    }

    #[test]
    fn key_codec_round_trips() {
        let k = key();
        assert_eq!(decode_key(&encode_key(&k)).unwrap(), k);
        assert!(decode_key("short").is_err());
    }
}
