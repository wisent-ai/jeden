//! Real-time collaboration relay: a content-agnostic HTTP message broker plus
//! end-to-end-encrypted client helpers. The relay stores only opaque base64
//! blobs per room and never sees plaintext or decryption keys (those live in
//! the client and travel in the `#key=` URL fragment, never over the wire).

use crate::tally::FIRST;
use aes_gcm::aead::generic_array::typenum::Unsigned;
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

mod client;
mod relay;

pub use client::{relay_get, relay_post_authorized};
pub use relay::serve;
pub(crate) use relay::SCHEMA as RELAY_SCHEMA;

/// A room's end-to-end key, of the length AES-256-GCM declares.
pub type RoomKey = Key<Aes256Gcm>;

/// The collaboration frame protocol this build speaks: its first version.
const FRAME_VERSION: u64 = FIRST;

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

    /// How this role is spelled in write tokens, URL fragments, relay
    /// headers and the relay's rows: its serde name, declared once above.
    pub fn name(self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(name)) => name,
            other => unreachable!("a unit role serializes as its name, not {other:?}"),
        }
    }

    /// The role spelled `name`, or nothing when no role is spelled that way.
    pub fn named(name: &str) -> Option<Self> {
        match serde_json::from_value(serde_json::Value::String(name.to_owned())) {
            Ok(role) => Some(role),
            Err(_unknown) => None,
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
    pub version: u64,
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
        let created_at = crate::clock::unix_millis()
            .map_err(|error| format!("the collaboration frame cannot be stamped: {error}"))?;
        Ok(Self {
            version: FRAME_VERSION,
            id: uuid::Uuid::new_v4().simple().to_string(),
            client_id: client_id.into(),
            created_at,
            role,
            kind,
        })
    }
}

pub fn seal_frame(key: &RoomKey, frame: &ProtocolFrame) -> Result<String, String> {
    let plain = serde_json::to_vec(frame)
        .map_err(|error| format!("the collab frame cannot be encoded: {error}"))?;
    encrypt_blob(key, &plain)
}

pub fn open_frame(key: &RoomKey, blob: &str) -> Result<ProtocolFrame, String> {
    let frame: ProtocolFrame = serde_json::from_slice(&decrypt_blob(key, blob)?)
        .map_err(|error| format!("the decrypted collab frame is not a protocol frame: {error}"))?;
    if frame.version != FRAME_VERSION {
        return Err(format!(
            "unsupported collab frame version {}; this build speaks {FRAME_VERSION}",
            frame.version
        ));
    }
    if !frame.role.permits(&frame.kind) && frame.role != CollabRole::Full {
        return Err("frame role does not permit its operation".into());
    }
    Ok(frame)
}

/// A write token for `role`: its name and a fresh secret as strong as a room key.
pub fn new_role_write_token(role: CollabRole) -> String {
    format!("{}.{}", role.name(), URL_SAFE_NO_PAD.encode(new_key()))
}

pub fn new_write_token() -> String {
    new_role_write_token(CollabRole::Full)
}

// ---------------------------------------------------------------------------
// E2EE blob helpers (pure, round-trippable)
// ---------------------------------------------------------------------------

/// Encrypt `plain` under `key`, returning `base64url(nonce || ciphertext+tag)`.
/// A fresh random nonce is generated per call, so identical plaintext yields
/// distinct blobs.
pub fn encrypt_blob(key: &RoomKey, plain: &[u8]) -> Result<String, String> {
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plain)
        .map_err(|error| format!("encryption under the room key failed: {error}"))?;
    let mut framed = nonce.to_vec();
    framed.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(framed))
}

/// Reverse of [`encrypt_blob`]. Rejects a blob too short to contain a nonce.
pub fn decrypt_blob(key: &RoomKey, blob: &str) -> Result<Vec<u8>, String> {
    let framed = URL_SAFE_NO_PAD
        .decode(blob.trim())
        .map_err(|error| format!("the encrypted blob is not base64url: {error}"))?;
    let (nonce, ciphertext) = framed
        .split_at_checked(<Aes256Gcm as AeadCore>::NonceSize::USIZE)
        .ok_or("encrypted blob is too short to hold its nonce")?;
    Aes256Gcm::new(key)
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|error| format!("decryption failed (wrong key or corrupt blob): {error}"))
}

/// Encode a room key for a `#key=` URL fragment.
pub fn encode_key(key: &RoomKey) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

/// Decode a `#key=` fragment back into a room key, refused when it is not one.
pub fn decode_key(text: &str) -> Result<RoomKey, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim())
        .map_err(|error| format!("the relay key is not base64url: {error}"))?;
    let length = bytes.len();
    RoomKey::from_exact_iter(bytes)
        .ok_or_else(|| format!("relay key is {length} bytes, not the length of an AES-256-GCM key"))
}

/// A freshly generated random room key.
pub fn new_key() -> RoomKey {
    Aes256Gcm::generate_key(OsRng)
}

/// A freshly generated random room id.
pub fn new_room() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------------------------------------------------------------------------
// Relay URL parsing (pure)
// ---------------------------------------------------------------------------

/// A parsed HTTP relay target: the server base (scheme, host and port), the
/// room when the URL names one, and what the `#` fragment carries.
pub struct RelayUrl {
    pub base: String,
    pub room: Option<String>,
    pub key: Option<RoomKey>,
    pub write_token: Option<String>,
    pub role: CollabRole,
}

/// Parse an HTTP relay URL of the form `http://host[:port][/room/<id>][#key=<k>&write=<t>&role=<r>]`.
/// A URL without a fragment, or without `role=`, is read for viewing.
pub fn parse_relay_url(text: &str) -> Result<RelayUrl, String> {
    let url =
        url::Url::parse(text.trim()).map_err(|error| format!("malformed relay URL: {error}"))?;
    if !url.has_host() || url.cannot_be_a_base() {
        return Err("relay URL is missing a host".into());
    }
    let mut key = None;
    let mut write_token = None;
    let mut role = CollabRole::View;
    if let Some(fragment) = url.fragment() {
        for pair in fragment.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or("relay fragment fields must be name=value")?;
            match name {
                "key" => key = Some(decode_key(value)?),
                "write" => write_token = Some(value.to_string()),
                "role" => {
                    role = CollabRole::named(value)
                        .ok_or_else(|| format!("unknown collaboration role: {value}"))?
                }
                _ => return Err(format!("unknown relay fragment field: {name}")),
            }
        }
    }
    let path = url.path().trim_matches('/');
    let room = match path.strip_prefix("room/") {
        Some(room) => Some(room.trim_matches('/').to_string()).filter(|room| !room.is_empty()),
        None => None,
    };
    Ok(RelayUrl {
        base: url.origin().ascii_serialization(),
        room,
        key,
        write_token,
        role,
    })
}
