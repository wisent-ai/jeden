use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as ShaDigest, Sha256};
use std::fmt;
use std::str::FromStr;

pub const SHA256_BYTES: usize = 32;
pub const SHA256_HEX_LEN: usize = SHA256_BYTES * 2;

/// A SHA-256 digest. Its textual and serde representation is 64 lowercase hex digits.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Digest([u8; SHA256_BYTES]);

impl Digest {
    pub const fn from_bytes(bytes: [u8; SHA256_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn of(bytes: impl AsRef<[u8]>) -> Self {
        let value = Sha256::digest(bytes.as_ref());
        let mut digest = [0_u8; SHA256_BYTES];
        digest.copy_from_slice(&value);
        Self(digest)
    }

    pub const fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
        &self.0
    }
    pub fn into_bytes(self) -> [u8; SHA256_BYTES] {
        self.0
    }
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DigestParseError {
    InvalidLength { actual: usize },
    InvalidHex,
}

impl fmt::Display for DigestParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { actual } => write!(
                formatter,
                "SHA-256 digest must contain {SHA256_HEX_LEN} hex digits, got {actual}"
            ),
            Self::InvalidHex => {
                formatter.write_str("SHA-256 digest contains non-hexadecimal characters")
            }
        }
    }
}
impl std::error::Error for DigestParseError {}

impl FromStr for Digest {
    type Err = DigestParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != SHA256_HEX_LEN {
            return Err(DigestParseError::InvalidLength {
                actual: value.len(),
            });
        }
        let bytes = hex::decode(value).map_err(|_| DigestParseError::InvalidHex)?;
        let mut digest = [0_u8; SHA256_BYTES];
        digest.copy_from_slice(&bytes);
        Ok(Self(digest))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}
impl fmt::Debug for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Digest({self})")
    }
}
impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}
