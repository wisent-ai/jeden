use serde::{Deserialize, Serialize};

use super::EvidenceReference;


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Read from a verifier's answer as well as from the state, so extra fields a
// model echoes are ignored; every field below is still required.
#[serde(rename_all = "camelCase")]
pub struct CriterionReview {
    pub index: usize,
    pub satisfied: bool,
    pub explanation: String,
    pub evidence: Vec<EvidenceReference>,
}
