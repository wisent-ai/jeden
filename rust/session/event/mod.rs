//! One sealed session event: what it says, who it follows, and the checksum
//! that proves it was not edited after the fact.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod legacy_kinds;
pub(crate) mod payload;

pub(crate) use payload::{CheckpointPayloadV2, RewindPayloadV2, SessionPayloadV2};

pub(crate) const SESSION_EVENT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionEventV2 {
    pub(crate) event_id: String,
    pub(crate) session_id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) sequence: u64,
    pub(crate) timestamp: String,
    pub(crate) causation_id: Option<String>,
    pub(crate) correlation_id: String,
    pub(crate) schema_version: u32,
    pub(crate) payload: SessionPayloadV2,
    #[serde(default)]
    pub(crate) outbox: Vec<super::outbox::OutboxItem>,
    pub(crate) checksum: String,
}

impl SessionEventV2 {
    pub(crate) fn seal(&mut self) -> Result<(), String> {
        self.checksum.clear();
        self.checksum = checksum(self)?;
        Ok(())
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        if self.schema_version != SESSION_EVENT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported session event schema version {}",
                self.schema_version
            ));
        }
        let mut unsigned = self.clone();
        let expected = std::mem::take(&mut unsigned.checksum);
        let actual = checksum(&unsigned)?;
        if expected != actual {
            return Err(format!("event {} checksum mismatch", self.event_id));
        }
        Ok(())
    }
}

fn checksum(event: &SessionEventV2) -> Result<String, String> {
    let bytes = serde_json::to_vec(event).map_err(|e| e.to_string())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
