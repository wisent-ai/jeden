//! The release channels an update follows, each spelled and keyed here once:
//! the update request, the trust roots it verifies against and the refusal of
//! an unknown channel all read this declaration.
use base64::Engine;
use ed25519_dalek::{VerifyingKey, PUBLIC_KEY_LENGTH};

use super::manifest::TrustRoot;

/// One release channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Canary,
    Stable,
}

impl Channel {
    const ALL: &'static [Channel] = &[Channel::Canary, Channel::Stable];

    /// The channel an update follows when none is declared, as the update
    /// command documents it.
    pub const DEFAULT: Channel = Channel::Stable;

    /// The channel's name, as a request, a manifest and a trust root spell it.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Canary => "canary",
            Channel::Stable => "stable",
        }
    }

    /// The channel `name` spells, read back through [`Channel::name`].
    pub fn named(name: &str) -> Option<Channel> {
        Self::ALL.iter().copied().find(|channel| channel.name() == name)
    }

    /// Every channel's name, for a refusal.
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|channel| channel.name()).collect()
    }

    /// The release key this channel's manifests are signed with: its id and
    /// its Ed25519 public key in base64.
    fn release_key(self) -> (&'static str, &'static str) {
        match self {
            Channel::Canary => ("jeden-canary-2026-07-13", "8hCBoR81Kax1U4oPKyg0C9IvYifV+o+6qc4L6JYbCFk="),
            Channel::Stable => ("jeden-stable-2026-07-13", "78wFp2XYBVMWv/MfkvTlQ3TqWjyHMgJWKA9KK4e9wsA="),
        }
    }
}

/// The trust root of every channel, from the release keys built in. A key
/// that does not decode is refused naming its channel and the decoder's cause.
pub fn embedded_trust_roots() -> Result<Vec<TrustRoot>, String> {
    Channel::ALL
        .iter()
        .map(|channel| {
            let (key_id, encoded) = channel.release_key();
            let name = channel.name();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| format!("the {name} trust root {key_id} is not base64: {error}"))?;
            let length = bytes.len();
            let bytes: [u8; PUBLIC_KEY_LENGTH] = bytes.try_into().map_err(|_unsized: Vec<u8>| {
                format!("the {name} trust root {key_id} holds {length} bytes, not an Ed25519 public key's {PUBLIC_KEY_LENGTH}")
            })?;
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|error| format!("the {name} trust root {key_id} is not an Ed25519 key: {error}"))?;
            Ok(TrustRoot {
                channel: name.into(),
                key_id: key_id.into(),
                key,
            })
        })
        .collect()
}
