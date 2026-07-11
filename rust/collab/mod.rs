//! Real-time collaboration relay: a content-agnostic HTTP message broker plus
//! end-to-end-encrypted client helpers. The relay stores only opaque base64
//! blobs per room and never sees plaintext or decryption keys (those live in
//! the client and travel in the `#key=` URL fragment, never over the wire).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;

mod client;
mod relay;

pub use client::{
    relay_get, relay_post, relay_post_authorized, relay_rotate_write_token, LiveClient,
};
pub use relay::serve;

/// Max size of a single relay blob (base64 E2EE payload). Rejects larger POSTs.
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;
/// Max buffered events per room before the relay applies backpressure.
pub const MAX_ROOM_EVENTS: usize = 10_000;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollabRole {
    View,
    Prompt,
    Abort,
    Full,
}

impl CollabRole {
    pub fn permits(self, kind: &FrameKind) -> bool {
        match self {
            Self::View => false,
            Self::Prompt => matches!(kind, FrameKind::Prompt { .. }),
            Self::Abort => matches!(kind, FrameKind::Abort { .. }),
            Self::Full => true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrameKind {
    Transcript {
        entry: serde_json::Value,
    },
    State {
        value: serde_json::Value,
    },
    Tool {
        tool_call_id: String,
        value: serde_json::Value,
    },
    Agent {
        agent_id: String,
        value: serde_json::Value,
    },
    Prompt {
        text: String,
    },
    Abort {
        operation_id: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolFrame {
    pub version: u32,
    pub id: String,
    pub client_id: String,
    pub created_at: u64,
    pub role: CollabRole,
    pub kind: FrameKind,
}

impl ProtocolFrame {
    pub fn new(
        client_id: impl Into<String>,
        role: CollabRole,
        kind: FrameKind,
    ) -> Result<Self, String> {
        if !role.permits(&kind) && role != CollabRole::Full {
            return Err("collaboration role does not permit this frame".into());
        }
        let mut id = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut id);
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        Ok(Self {
            version: 1,
            id: hex::encode(id),
            client_id: client_id.into(),
            created_at,
            role,
            kind,
        })
    }
}

pub fn seal_frame(key: &[u8; 32], frame: &ProtocolFrame) -> Result<String, String> {
    encrypt_blob(key, &serde_json::to_vec(frame).map_err(|e| e.to_string())?)
}

pub fn open_frame(key: &[u8; 32], blob: &str) -> Result<ProtocolFrame, String> {
    let frame: ProtocolFrame =
        serde_json::from_slice(&decrypt_blob(key, blob)?).map_err(|e| e.to_string())?;
    if frame.version != 1 {
        return Err(format!(
            "unsupported collab frame version {}",
            frame.version
        ));
    }
    if !frame.role.permits(&frame.kind) && frame.role != CollabRole::Full {
        return Err("frame role does not permit its operation".into());
    }
    Ok(frame)
}

pub fn new_role_write_token(role: CollabRole) -> String {
    let mut token = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut token);
    let role = match role {
        CollabRole::View => "view",
        CollabRole::Prompt => "prompt",
        CollabRole::Abort => "abort",
        CollabRole::Full => "full",
    };
    format!("{role}.{}", URL_SAFE_NO_PAD.encode(token))
}

pub fn new_write_token() -> String {
    new_role_write_token(CollabRole::Full)
}

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
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim())
        .map_err(|e| e.to_string())?;
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
pub struct RelayUrl {
    pub base: String,
    pub room: String,
    pub key: Option<[u8; 32]>,
    pub write_token: Option<String>,
    pub role: CollabRole,
}

/// Parse an HTTP relay URL of the form `http://host[:port][/room/<id>][#key=<k>]`.
/// A missing room yields an empty `room` (caller generates one for `start`).
pub fn parse_relay_url(text: &str) -> Result<RelayUrl, String> {
    let text = text.trim();
    if !(text.starts_with("http://") || text.starts_with("https://")) {
        return Err("relay URL must start with http:// or https://".into());
    }
    let (without_frag, key, write_token, role) = match text.split_once('#') {
        Some((head, frag)) => {
            let mut key = None;
            let mut write_token = None;
            let mut role = CollabRole::View;
            for pair in frag.split('&') {
                let (name, value) = pair
                    .split_once('=')
                    .ok_or("relay fragment fields must be name=value")?;
                match name {
                    "key" => key = Some(decode_key(value)?),
                    "write" => write_token = Some(value.to_string()),
                    "role" => {
                        role = match value {
                            "view" => CollabRole::View,
                            "prompt" => CollabRole::Prompt,
                            "abort" => CollabRole::Abort,
                            "full" => CollabRole::Full,
                            _ => return Err("unknown collaboration role".into()),
                        }
                    }
                    _ => return Err(format!("unknown relay fragment field: {name}")),
                }
            }
            (head, key, write_token, role)
        }
        None => (text, None, None, CollabRole::View),
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
    Ok(RelayUrl {
        base,
        room,
        key,
        write_token,
        role,
    })
}
